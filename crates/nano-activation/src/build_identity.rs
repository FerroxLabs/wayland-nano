//! Compile-derived source identity. The executable digest is supplied independently by its runner.

use crate::receipt::ArtifactIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledBuildIdentity {
    pub source_commit_sha: &'static str,
    pub cargo_lock_sha256: &'static str,
    pub source_dirty: bool,
}

pub fn compiled() -> CompiledBuildIdentity {
    CompiledBuildIdentity {
        source_commit_sha: env!("NANO_SOURCE_COMMIT_SHA"),
        cargo_lock_sha256: env!("NANO_CARGO_LOCK_SHA256"),
        source_dirty: matches!(env!("NANO_SOURCE_DIRTY"), "true"),
    }
}

impl CompiledBuildIdentity {
    pub fn bind_executable(
        &self,
        executable_sha256: &str,
    ) -> Result<ArtifactIdentity, BuildIdentityError> {
        if self.source_commit_sha.len() != 40
            || !hex(self.source_commit_sha)
            || self.cargo_lock_sha256.len() != 64
            || !hex(self.cargo_lock_sha256)
            || executable_sha256.len() != 64
            || !hex(executable_sha256)
        {
            return Err(BuildIdentityError::Invalid);
        }
        if self.source_dirty && !cfg!(debug_assertions) {
            return Err(BuildIdentityError::DirtySource);
        }
        Ok(ArtifactIdentity {
            source_commit_sha: self.source_commit_sha.into(),
            cargo_lock_sha256: self.cargo_lock_sha256.into(),
            executable_sha256: executable_sha256.into(),
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BuildIdentityError {
    #[error("compiled build identity is malformed")]
    Invalid,
    #[error("release build identity came from a dirty source tree")]
    DirtySource,
}

fn hex(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
