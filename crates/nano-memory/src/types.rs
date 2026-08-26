use serde::{Deserialize, Serialize};

pub type MemoryResult<T> = Result<T, MemoryError>;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("journal: {0}")]
    Journal(#[from] std::io::Error),
    #[error("journal integrity: {0}")]
    JournalIntegrity(String),
    #[error("invalid {field}: {value}")]
    InvalidValue { field: &'static str, value: String },
    #[error("memory policy disables this operation")]
    Disabled,
    #[error("model writes require host mediation")]
    MediationRequired,
    #[error("deterministic write screening rejected the proposal")]
    ScreeningRejected,
    #[error("network filesystem memory paths are refused")]
    NetworkFilesystem,
    #[error("memory writer contention: {0}")]
    Contention(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceTrust {
    User,
    ToolOutput,
    ModelInference,
}
impl SourceTrust {
    pub const fn rank(self) -> u8 {
        match self {
            Self::User => 3,
            Self::ToolOutput => 2,
            Self::ModelInference => 1,
        }
    }
    pub const fn weight(self) -> f64 {
        match self {
            Self::User => 1.0,
            Self::ToolOutput => 0.8,
            Self::ModelInference => 0.5,
        }
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::ToolOutput => "ToolOutput",
            Self::ModelInference => "ModelInference",
        }
    }
    pub fn parse(value: &str) -> MemoryResult<Self> {
        match value {
            "User" => Ok(Self::User),
            "ToolOutput" => Ok(Self::ToolOutput),
            "ModelInference" => Ok(Self::ModelInference),
            _ => Err(MemoryError::InvalidValue {
                field: "source_trust",
                value: value.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentScope {
    Own,
    OwnAndProject,
    Explicit(Vec<String>),
}
impl AgentScope {
    pub fn parse(value: &str) -> MemoryResult<Self> {
        match value {
            "Own" => Ok(Self::Own),
            "OwnAndProject" => Ok(Self::OwnAndProject),
            _ => Err(MemoryError::InvalidValue {
                field: "agent_scope",
                value: value.into(),
            }),
        }
    }
    pub(crate) fn ids(&self, querying: &str) -> Vec<String> {
        match self {
            Self::Own => vec![querying.into()],
            Self::OwnAndProject => {
                if querying == "main" {
                    vec!["main".into()]
                } else {
                    vec![querying.into(), "main".into()]
                }
            }
            Self::Explicit(ids) => ids.clone(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadScope {
    Session,
    SessionAndProject,
}
impl ReadScope {
    pub fn parse(value: &str) -> MemoryResult<Self> {
        match value {
            "Session" => Ok(Self::Session),
            "SessionAndProject" => Ok(Self::SessionAndProject),
            _ => Err(MemoryError::InvalidValue {
                field: "read_scope",
                value: value.into(),
            }),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteScope {
    Off,
    SessionOnly,
    SessionAndProject,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedderChoice {
    HashedLocal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionRule {
    Never,
    HardDelete,
}
#[derive(Debug, Clone, Copy)]
pub struct RetentionCaps {
    pub episodes: u64,
    pub facts: u64,
    pub bytes: u64,
}
impl Default for RetentionCaps {
    fn default() -> Self {
        Self {
            episodes: 10_000,
            facts: 50_000,
            bytes: 256 * 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone)]
pub struct MemoryPolicy {
    pub enabled: bool,
    pub write: WriteScope,
    pub read_scope: ReadScope,
    pub retention: RetentionCaps,
    pub embedding_backend: EmbedderChoice,
    pub deletion: DeletionRule,
    pub min_tier: SourceTrust,
}
impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            write: WriteScope::SessionAndProject,
            read_scope: ReadScope::SessionAndProject,
            retention: RetentionCaps::default(),
            embedding_backend: EmbedderChoice::HashedLocal,
            deletion: DeletionRule::Never,
            min_tier: SourceTrust::ModelInference,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactWrite {
    pub id: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64,
    pub source_episode: Option<String>,
    pub valid_from: String,
    pub valid_to: Option<String>,
    pub source_trust: SourceTrust,
    pub project: String,
    pub agent_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionWrite {
    pub id: String,
    pub summary: String,
    pub why: String,
    pub how_to_apply: String,
    pub tags: Vec<String>,
    pub source_episode: Option<String>,
    #[serde(default = "default_valid_from")]
    pub valid_from: String,
    #[serde(default)]
    pub valid_to: Option<String>,
    pub source_trust: SourceTrust,
    pub project: String,
    pub agent_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeWrite {
    pub id: String,
    pub content: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_source_product")]
    pub source_product: String,
    pub valid_from: String,
    pub valid_to: Option<String>,
    pub source_trust: SourceTrust,
    pub project: String,
    pub agent_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureWrite {
    pub id: String,
    pub title: String,
    pub steps: String,
    pub created_by: String,
    #[serde(default = "default_valid_from")]
    pub valid_from: String,
    #[serde(default)]
    pub valid_to: Option<String>,
    pub source_trust: SourceTrust,
    pub project: String,
    pub agent_id: String,
}
fn default_valid_from() -> String {
    "1970-01-01T00:00:00Z".into()
}
fn default_source() -> String {
    "host".into()
}
fn default_source_product() -> String {
    "wayland-nano".into()
}
#[derive(Debug, Clone)]
pub struct RetrieveQuery {
    pub text: String,
    pub project: String,
    pub agent_id: String,
    pub agent_scope: AgentScope,
    pub limit: usize,
    pub token_budget: usize,
    pub min_tier: SourceTrust,
}
#[derive(Debug, Clone, PartialEq)]
pub struct RetrieveHit {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub score: f64,
    pub confidence: f64,
    pub source_episode: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub source_trust: SourceTrust,
    pub project: String,
    pub agent_id: String,
}
#[derive(Debug, Clone, PartialEq)]
pub struct FactState {
    pub id: String,
    pub valid_from: String,
    pub valid_to: Option<String>,
    pub source_trust: SourceTrust,
    pub project: String,
    pub agent_id: String,
}

pub(crate) fn reject_network_path(path: &std::path::Path) -> MemoryResult<()> {
    let text = path.to_string_lossy();
    if text.starts_with("\\\\") || text.starts_with("//") {
        return Err(MemoryError::NetworkFilesystem);
    }
    Ok(())
}
pub(crate) fn memory_db_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("memory").join("memory.db")
}
pub(crate) fn validate_partition(project: &str, agent: &str) -> MemoryResult<()> {
    if project.is_empty() {
        return Err(MemoryError::InvalidValue {
            field: "project",
            value: project.into(),
        });
    }
    let valid = agent.len() <= 64
        && agent.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && agent
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !valid {
        return Err(MemoryError::InvalidValue {
            field: "agent_id",
            value: agent.into(),
        });
    }
    Ok(())
}
