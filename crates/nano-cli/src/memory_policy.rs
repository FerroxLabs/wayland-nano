//! Host-side MemoryPolicy source (03-02; owner-approved decision 2026-09-02,
//! PR #21 amendment): strict `$NANO_HOME/memory-policy.toml` with
//! deny-unknown-fields, resolved into the frozen nano-memory `MemoryPolicy`
//! type, plus the MEMORY-CONTRACT §6.8 config-file agent registry
//! (`$NANO_HOME/agents/*.agent.toml`).
//!
//! Posture: default-off (absent or empty source resolves `enabled: false`),
//! tighten-only (every knob must be declared explicitly; nothing inherits the
//! widened `MemoryPolicy::default()`), and fail-closed (unknown fields,
//! `read_scope = "Global"`, unknown `min_tier`/`source_trust`/`agent_scope`
//! values, malformed/duplicate/stem-mismatched agent files are all typed
//! errors, never coerced). The legacy `NANO_MEMORY_WRITE` /
//! `NANO_MEMORY_BLOCK_CHARS` env surface for the quarantined `.md` store is a
//! different, untouched channel (03-04's migration target).
//!
//! This module resolves only. It never opens the memory store or a journal:
//! `MemoryStore`'s `validate_policy` remains the real fail-closed gate and
//! `Op::MemoryPolicyResolved` journaling is 03-03's seam obligation.

use nano_memory::{ConfiguredAgents, MemoryPolicy};
use std::path::{Path, PathBuf};

/// The one typed handle every persistent entrypoint resolves at startup.
/// 03-03's seam consumes `policy()` and `configured_agents()` for the real
/// store-open validation and the journaled policy record.
#[derive(Debug, Clone)]
pub struct ResolvedMemoryPolicy {
    policy: MemoryPolicy,
    agents: ConfiguredAgents,
}

impl ResolvedMemoryPolicy {
    /// The default-off resolution: master switch off (no reads, no writes),
    /// tightest write scope, only the implicit `main` configured agent.
    pub fn disabled() -> Self {
        Self {
            policy: MemoryPolicy {
                enabled: false,
                write: nano_memory::WriteScope::Off,
                read_scope: nano_memory::ReadScope::SessionAndProject,
                retention: nano_memory::RetentionCaps::default(),
                embedding_backend: nano_memory::EmbedderChoice::HashedLocal,
                deletion: nano_memory::DeletionRule::Never,
                min_tier: nano_memory::SourceTrust::ModelInference,
                session_id: None,
            },
            agents: ConfiguredAgents::try_from_ids(std::iter::empty())
                .expect("the implicit-main set is always valid"),
        }
    }

    pub fn policy(&self) -> &MemoryPolicy {
        &self.policy
    }

    pub fn configured_agents(&self) -> &ConfiguredAgents {
        &self.agents
    }
}

#[derive(Debug)]
pub enum MemoryPolicyError {
    Io { path: PathBuf, message: String },
    Parse { path: PathBuf, message: String },
    InvalidAgentId { id: String, message: String },
    DuplicateAgent { id: String },
    ReservedAgent { id: String },
    AgentStemMismatch { path: PathBuf, declared: String },
    EmptyAgentFileName { path: PathBuf },
}

impl std::fmt::Display for MemoryPolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(
                    formatter,
                    "memory policy source {} unreadable: {message}",
                    path.display()
                )
            }
            Self::Parse { path, message } => {
                write!(
                    formatter,
                    "memory policy source {} invalid: {message}",
                    path.display()
                )
            }
            Self::InvalidAgentId { id, message } => {
                write!(
                    formatter,
                    "configured memory agent id `{id}` invalid: {message}"
                )
            }
            Self::DuplicateAgent { id } => {
                write!(
                    formatter,
                    "configured memory agent `{id}` declared more than once"
                )
            }
            Self::ReservedAgent { id } => {
                write!(
                    formatter,
                    "configured memory agent id `{id}` is reserved (implicit)"
                )
            }
            Self::AgentStemMismatch { path, declared } => write!(
                formatter,
                "configured memory agent file {} declares id `{declared}`, which must match its filename stem",
                path.display()
            ),
            Self::EmptyAgentFileName { path } => write!(
                formatter,
                "configured memory agent file {} has an empty id stem",
                path.display()
            ),
        }
    }
}

impl std::error::Error for MemoryPolicyError {}

/// Resolve the host memory policy and the §6.8 configured-agent registry
/// from `$NANO_HOME`. Absent/empty sources resolve default-off with only the
/// implicit `main` agent; any malformed content is a typed error.
pub fn resolve(nano_home: &Path) -> Result<ResolvedMemoryPolicy, MemoryPolicyError> {
    Ok(ResolvedMemoryPolicy {
        policy: load_policy(nano_home)?,
        agents: load_configured_agents(nano_home)?,
    })
}

/// The strict on-disk policy document. Every knob is mandatory (tighten-only:
/// an omitted field is a typed error, never the widened crate default) and
/// unknown fields are refused. Field-for-field the frozen nano-memory
/// `MemoryPolicy`; conversion below never invents a value.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    enabled: bool,
    write: nano_memory::WriteScope,
    read_scope: nano_memory::ReadScope,
    retention: RetentionFile,
    embedding_backend: nano_memory::EmbedderChoice,
    deletion: nano_memory::DeletionRule,
    min_tier: nano_memory::SourceTrust,
    session_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionFile {
    episodes: u64,
    facts: u64,
    bytes: u64,
}

impl From<PolicyFile> for MemoryPolicy {
    fn from(file: PolicyFile) -> Self {
        Self {
            enabled: file.enabled,
            write: file.write,
            read_scope: file.read_scope,
            retention: nano_memory::RetentionCaps {
                episodes: file.retention.episodes,
                facts: file.retention.facts,
                bytes: file.retention.bytes,
            },
            embedding_backend: file.embedding_backend,
            deletion: file.deletion,
            min_tier: file.min_tier,
            session_id: file.session_id,
        }
    }
}

/// §6.8 registry entry: the declared id only. `main` is reserved (it is the
/// implicit configured orchestrator per the frozen `ConfiguredAgents`).
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentFile {
    id: String,
}

const AGENT_FILE_SUFFIX: &str = ".agent.toml";

fn load_policy(nano_home: &Path) -> Result<MemoryPolicy, MemoryPolicyError> {
    let path = nano_home.join("memory-policy.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ResolvedMemoryPolicy::disabled().policy);
        }
        Err(error) => {
            return Err(MemoryPolicyError::Io {
                path,
                message: error.to_string(),
            });
        }
    };
    if text.trim().is_empty() {
        return Ok(ResolvedMemoryPolicy::disabled().policy);
    }
    let parsed: PolicyFile = toml::from_str(&text).map_err(|error| MemoryPolicyError::Parse {
        path: path.clone(),
        message: error.to_string(),
    })?;
    Ok(parsed.into())
}

fn load_configured_agents(nano_home: &Path) -> Result<ConfiguredAgents, MemoryPolicyError> {
    let dir = nano_home.join("agents");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ConfiguredAgents::try_from_ids(std::iter::empty()).map_err(invalid_agent);
        }
        Err(error) => {
            return Err(MemoryPolicyError::Io {
                path: dir,
                message: error.to_string(),
            });
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| MemoryPolicyError::Io {
            path: dir.clone(),
            message: error.to_string(),
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(AGENT_FILE_SUFFIX) {
            files.push((entry.path(), name));
        }
    }
    files.sort();
    let mut declared: Vec<String> = Vec::with_capacity(files.len());
    for (path, name) in files {
        let stem = &name[..name.len() - AGENT_FILE_SUFFIX.len()];
        if stem.is_empty() {
            return Err(MemoryPolicyError::EmptyAgentFileName { path });
        }
        let text = std::fs::read_to_string(&path).map_err(|error| MemoryPolicyError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let parsed: AgentFile =
            toml::from_str(&text).map_err(|error| MemoryPolicyError::Parse {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if parsed.id == "main" {
            return Err(MemoryPolicyError::ReservedAgent { id: parsed.id });
        }
        if declared.contains(&parsed.id) {
            return Err(MemoryPolicyError::DuplicateAgent { id: parsed.id });
        }
        if parsed.id != stem {
            return Err(MemoryPolicyError::AgentStemMismatch {
                path,
                declared: parsed.id,
            });
        }
        declared.push(parsed.id);
    }
    ConfiguredAgents::try_from_ids(declared).map_err(invalid_agent)
}

fn invalid_agent(error: nano_memory::MemoryError) -> MemoryPolicyError {
    let message = error.to_string();
    match error {
        nano_memory::MemoryError::InvalidValue { value, .. } => {
            MemoryPolicyError::InvalidAgentId { id: value, message }
        }
        _ => MemoryPolicyError::InvalidAgentId {
            id: String::new(),
            message,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_memory::{ReadScope, SourceTrust, WriteScope};

    const VALID_POLICY: &str = "\
enabled = true
write = \"SessionOnly\"
read_scope = \"Session\"
embedding_backend = \"HashedLocal\"
deletion = \"Never\"
min_tier = \"User\"
session_id = \"session-a\"

[retention]
episodes = 100
facts = 200
bytes = 4096
";

    fn write_policy(home: &Path, text: &str) {
        std::fs::write(home.join("memory-policy.toml"), text).unwrap();
    }

    fn write_agent(home: &Path, file_name: &str, text: &str) {
        let dir = home.join("agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file_name), text).unwrap();
    }

    #[test]
    fn absent_source_resolves_disabled_default_off() {
        let home = tempfile::tempdir().unwrap();
        let resolved = resolve(home.path()).unwrap();
        assert!(!resolved.policy().enabled);
        assert_eq!(resolved.policy().write, WriteScope::Off);
        assert!(resolved.configured_agents().contains("main"));
        assert!(!resolved.configured_agents().contains("bot-a"));
    }

    #[test]
    fn empty_or_whitespace_source_resolves_disabled() {
        for text in ["", "  \n\t\n"] {
            let home = tempfile::tempdir().unwrap();
            write_policy(home.path(), text);
            let resolved = resolve(home.path()).unwrap();
            assert!(!resolved.policy().enabled, "source {text:?}");
        }
    }

    #[test]
    fn valid_source_resolves_the_exact_typed_policy() {
        let home = tempfile::tempdir().unwrap();
        write_policy(home.path(), VALID_POLICY);
        let resolved = resolve(home.path()).unwrap();
        let policy = resolved.policy();
        assert!(policy.enabled);
        assert_eq!(policy.write, WriteScope::SessionOnly);
        assert_eq!(policy.read_scope, ReadScope::Session);
        assert_eq!(policy.retention.episodes, 100);
        assert_eq!(policy.retention.facts, 200);
        assert_eq!(policy.retention.bytes, 4096);
        assert_eq!(policy.min_tier, SourceTrust::User);
        assert_eq!(policy.deletion, nano_memory::DeletionRule::Never);
        assert_eq!(
            policy.embedding_backend,
            nano_memory::EmbedderChoice::HashedLocal
        );
        assert_eq!(policy.session_id.as_deref(), Some("session-a"));
    }

    #[test]
    fn unknown_top_level_field_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_policy(home.path(), &format!("bogus = 1\n{VALID_POLICY}"));
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    #[test]
    fn unknown_retention_field_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_policy(
            home.path(),
            &VALID_POLICY.replace("bytes = 4096", "bytes = 4096\nsurprise = 1"),
        );
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    // MEM-SEC-5 config-case compatibility (fixture mem-sec-5): removed and
    // unknown scope/trust vocabulary is a typed parse error, never coerced.
    #[test]
    fn removed_global_read_scope_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_policy(
            home.path(),
            &VALID_POLICY.replace("read_scope = \"Session\"", "read_scope = \"Global\""),
        );
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    #[test]
    fn unknown_min_tier_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_policy(
            home.path(),
            &VALID_POLICY.replace("min_tier = \"User\"", "min_tier = \"Administrator\""),
        );
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    #[test]
    fn legacy_source_trust_field_name_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_policy(
            home.path(),
            &format!("source_trust = \"User\"\n{VALID_POLICY}"),
        );
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    #[test]
    fn agent_scope_field_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_policy(
            home.path(),
            &format!("agent_scope = \"All\"\n{VALID_POLICY}"),
        );
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    // Tighten-only: a partial document never inherits the widened
    // `MemoryPolicy::default()` (which is enabled) — omitted knobs fail closed.
    #[test]
    fn omitted_knobs_never_inherit_the_widened_crate_default() {
        let home = tempfile::tempdir().unwrap();
        write_policy(
            home.path(),
            &VALID_POLICY.replace("write = \"SessionOnly\"\n", ""),
        );
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    #[test]
    fn malformed_toml_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_policy(home.path(), "enabled = [not a bool");
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    #[test]
    fn configured_agent_with_matching_stem_is_registered() {
        let home = tempfile::tempdir().unwrap();
        write_agent(home.path(), "bot-a.agent.toml", "id = \"bot-a\"\n");
        let resolved = resolve(home.path()).unwrap();
        assert!(resolved.configured_agents().contains("bot-a"));
        assert!(resolved.configured_agents().contains("main"));
        assert!(!resolved.configured_agents().contains("bot-z"));
    }

    #[test]
    fn configured_agent_stem_mismatch_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_agent(home.path(), "bot-a.agent.toml", "id = \"bot-b\"\n");
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::AgentStemMismatch { .. }
        ));
    }

    #[test]
    fn reserved_main_agent_file_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_agent(home.path(), "main.agent.toml", "id = \"main\"\n");
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::ReservedAgent { .. }
        ));
    }

    #[test]
    fn duplicate_agent_id_is_a_typed_error() {
        // Two files declaring the same id: the duplicate is rejected before
        // any stem discipline is consulted.
        let home = tempfile::tempdir().unwrap();
        write_agent(home.path(), "bot-a.agent.toml", "id = \"bot-a\"\n");
        write_agent(home.path(), "copy.agent.toml", "id = \"bot-a\"\n");
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::DuplicateAgent { .. }
        ));
    }

    #[test]
    fn malformed_agent_file_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_agent(home.path(), "bot-a.agent.toml", "id = [nope");
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    #[test]
    fn unknown_agent_field_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_agent(
            home.path(),
            "bot-a.agent.toml",
            "id = \"bot-a\"\npersona = \"friendly\"\n",
        );
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Parse { .. }
        ));
    }

    #[test]
    fn invalid_agent_id_grammar_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_agent(home.path(), "Bot-A.agent.toml", "id = \"Bot-A\"\n");
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::InvalidAgentId { .. }
        ));
    }

    #[test]
    fn empty_agent_file_stem_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        write_agent(home.path(), ".agent.toml", "id = \"main\"\n");
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::EmptyAgentFileName { .. }
        ));
    }

    #[test]
    fn unreadable_policy_source_is_a_typed_error() {
        let home = tempfile::tempdir().unwrap();
        // A directory at the policy path is not a readable document.
        std::fs::create_dir_all(home.path().join("memory-policy.toml")).unwrap();
        assert!(matches!(
            resolve(home.path()).unwrap_err(),
            MemoryPolicyError::Io { .. }
        ));
    }
}
