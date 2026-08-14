use thiserror::Error;

pub type CuaResult<T> = Result<T, CuaError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuaErrorKind {
    PolicyDenied,
    FocusLost,
    OsPermissionDenied,
    BackendUnavailable,
    CoordinateOutOfRange,
    Backend,
    /// Maps to the integrator's existing `UserCancelled` journal kind.
    Cancelled,
}

#[derive(Debug, Error)]
pub enum CuaError {
    #[error("computer-use policy denied the operation")]
    PolicyDenied,
    #[error("frontmost application changed before dispatch")]
    FocusLost,
    #[error("operating-system computer-use permission denied: {remedy}")]
    OsPermissionDenied { remedy: &'static str },
    #[error("computer-use backend unavailable: {reason}")]
    BackendUnavailable { reason: &'static str },
    #[error("coordinate is outside the primary display")]
    CoordinateOutOfRange,
    #[error("computer-use backend operation failed")]
    Backend,
    #[error("computer-use operation was cancelled")]
    Cancelled,
    #[error("invalid computer-use input")]
    InvalidInput,
}

impl CuaError {
    pub fn kind(&self) -> CuaErrorKind {
        match self {
            Self::PolicyDenied | Self::InvalidInput => CuaErrorKind::PolicyDenied,
            Self::FocusLost => CuaErrorKind::FocusLost,
            Self::OsPermissionDenied { .. } => CuaErrorKind::OsPermissionDenied,
            Self::BackendUnavailable { .. } => CuaErrorKind::BackendUnavailable,
            Self::CoordinateOutOfRange => CuaErrorKind::CoordinateOutOfRange,
            Self::Backend => CuaErrorKind::Backend,
            Self::Cancelled => CuaErrorKind::Cancelled,
        }
    }
}
