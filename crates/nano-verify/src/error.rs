//! Crate-local failures for verification infrastructure.

/// Failures in verification infrastructure rather than receipt-content verdicts.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The receipt store could not be accessed or updated.
    #[error("receipt store I/O: {0}")]
    StoreIo(#[source] std::io::Error),
    /// A generated artifact could not be persisted.
    #[error("artifact write failed: {0}")]
    Artifact(#[source] std::io::Error),
    /// A model call failed without exposing request contents.
    #[error("model call failed: {0}")]
    Generate(String),
    /// The gate registry is malformed, inconsistent, or unavailable.
    #[error("gate registry: {0}")]
    Registry(String),
    /// The bounded writer-lock acquisition budget was exhausted.
    #[error("writer lock held: {0}")]
    LockHeld(String),
}
