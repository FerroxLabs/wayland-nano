use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("invalid plugin data: {0}")]
    Invalid(String),
    #[error("plugin document exceeds {limit} bytes")]
    TooLarge { limit: usize },
    #[error("plugin path is unsafe: {0}")]
    UnsafePath(PathBuf),
    #[error("HTTP MCP plugins are unavailable in v1")]
    HttpTransportRefused,
    #[error("plugin store is busy")]
    PluginStoreBusy,
    #[error("plugin pin mismatch for {plugin}: expected {expected}, observed {observed}")]
    PinMismatch {
        plugin: String,
        expected: String,
        observed: String,
    },
    #[error("plugin is already installed: {0}")]
    AlreadyInstalled(String),
    #[error("plugin not found: {0}")]
    NotFound(String),
    #[error("plugin name is ambiguous: {0}")]
    Ambiguous(String),
    #[error("interactive consent required; pass --yes in non-interactive use")]
    ConsentRequired,
    #[error("plugin archive exceeds a configured bound")]
    ArchiveBound,
    #[error("plugin store I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("plugin JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plugin egress failed: {0}")]
    Egress(#[from] nano_egress::client::EgressError),
    #[error("plugin HTTP transport failed: {0}")]
    Http(String),
}

impl PluginError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
