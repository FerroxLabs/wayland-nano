use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
    #[error("unsupported memory policy: {0}")]
    UnsupportedPolicy(String),
    #[error("unconfigured memory agent: {0}")]
    UnconfiguredAgent(String),
}

#[derive(Debug, Clone)]
pub struct ConfiguredAgents {
    ids: HashSet<String>,
}
impl ConfiguredAgents {
    pub fn try_from_ids(ids: impl IntoIterator<Item = String>) -> MemoryResult<Self> {
        let mut configured = HashSet::from(["main".to_owned()]);
        for id in ids {
            validate_partition("configured-agent", &id)?;
            configured.insert(id);
        }
        Ok(Self { ids: configured })
    }

    pub fn contains(&self, agent_id: &str) -> bool {
        self.ids.contains(agent_id)
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteScope {
    Off,
    SessionOnly,
    SessionAndProject,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmbedderChoice {
    HashedLocal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletionRule {
    Never,
    HardDelete,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPolicy {
    pub enabled: bool,
    pub write: WriteScope,
    pub read_scope: ReadScope,
    pub retention: RetentionCaps,
    pub embedding_backend: EmbedderChoice,
    pub deletion: DeletionRule,
    pub min_tier: SourceTrust,
    /// Required context for narrowed session-only reads or writes. It is
    /// journaled with the resolved policy and never inferred from a path.
    pub session_id: Option<String>,
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
            session_id: None,
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
pub struct RetrievalEvidence {
    pub fts_hits: usize,
    pub knn_hits: usize,
    pub assembled: Vec<RetrieveHit>,
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
    platform_reject_network_path(path)
}

fn existing_canonical_ancestor(path: &std::path::Path) -> MemoryResult<std::path::PathBuf> {
    let mut candidate = path;
    while !candidate.exists() {
        candidate = candidate.parent().ok_or(MemoryError::NetworkFilesystem)?;
    }
    candidate
        .canonicalize()
        .map_err(|_| MemoryError::NetworkFilesystem)
}

#[cfg(windows)]
fn platform_reject_network_path(path: &std::path::Path) -> MemoryResult<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};

    let canonical = existing_canonical_ancestor(path)?;
    let input: Vec<u16> = canonical.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut root = vec![0u16; 32_768];
    // Resolving the existing ancestor first prevents a local-looking reparse point
    // from hiding a UNC target. GetVolumePathNameW also handles extended UNC paths.
    if unsafe { GetVolumePathNameW(input.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return Err(MemoryError::NetworkFilesystem);
    }
    let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };
    // WinBase.h constants; windows-sys 0.52 does not expose them in this feature set.
    if matches!(drive_type, 3 | 6) {
        Ok(())
    } else {
        Err(MemoryError::NetworkFilesystem)
    }
}

#[cfg(target_os = "linux")]
fn platform_reject_network_path(path: &std::path::Path) -> MemoryResult<()> {
    use std::os::unix::ffi::OsStrExt;

    let canonical = existing_canonical_ancestor(path)?;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|_| MemoryError::NetworkFilesystem)?;
    let mut best: Option<(usize, &str)> = None;
    for line in mountinfo.lines() {
        let (left, right) = line
            .split_once(" - ")
            .ok_or(MemoryError::NetworkFilesystem)?;
        let fields = left.split_whitespace().collect::<Vec<_>>();
        let fs_fields = right.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 5 || fs_fields.is_empty() {
            return Err(MemoryError::NetworkFilesystem);
        }
        let mount = unescape_mountinfo(fields[4]);
        let mount_path = std::path::Path::new(std::ffi::OsStr::from_bytes(&mount));
        if canonical.starts_with(mount_path)
            && best.as_ref().is_none_or(|(len, _)| mount.len() > *len)
        {
            best = Some((mount.len(), fs_fields[0]));
        }
    }
    let (_, fs_type) = best.ok_or(MemoryError::NetworkFilesystem)?;
    if is_network_fs(fs_type) {
        Err(MemoryError::NetworkFilesystem)
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn unescape_mountinfo(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4].iter().all(u8::is_ascii_digit)
        {
            let octal =
                (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + bytes[i + 3] - b'0';
            out.push(octal);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

#[cfg(all(unix, not(target_os = "linux")))]
fn platform_reject_network_path(path: &std::path::Path) -> MemoryResult<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let canonical = existing_canonical_ancestor(path)?;
    let c_path = CString::new(canonical.as_os_str().as_bytes())
        .map_err(|_| MemoryError::NetworkFilesystem)?;
    let mut info = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    if unsafe { libc::statfs(c_path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(MemoryError::NetworkFilesystem);
    }
    let info = unsafe { info.assume_init() };
    let raw = info.f_fstypename;
    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    let bytes = raw[..len].iter().map(|&c| c as u8).collect::<Vec<_>>();
    let fs_type = std::str::from_utf8(&bytes).map_err(|_| MemoryError::NetworkFilesystem)?;
    if is_network_fs(fs_type) {
        Err(MemoryError::NetworkFilesystem)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn is_network_fs(fs_type: &str) -> bool {
    matches!(
        fs_type.to_ascii_lowercase().as_str(),
        "9p" | "afs"
            | "cifs"
            | "davfs"
            | "fuse.sshfs"
            | "gcsfuse"
            | "lustre"
            | "ncpfs"
            | "nfs"
            | "nfs4"
            | "smbfs"
            | "sshfs"
            | "ceph"
            | "glusterfs"
    )
}

#[cfg(not(any(unix, windows)))]
fn platform_reject_network_path(_path: &std::path::Path) -> MemoryResult<()> {
    Err(MemoryError::NetworkFilesystem)
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
