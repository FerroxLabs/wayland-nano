//! Validated absolute path wrapper.
//!
//! Provenance: minimal port of Codex `codex-rs/utils/absolute-path/src/lib.rs`
//! @ 646f7c0a, reduced to the surface Nano consumers name
//! (`from_absolute_path`, `resolve_path_against_base`, `join`, `parent`,
//! `as_path`, `into_path_buf`, `to_path_buf`, `canonicalize`). Dropped:
//! thread-local deserialization guard, JsonSchema/TS derives.
//! Transformations: module path; `normalize_path_for_platform` inlined and
//! extended to reconstruct canonical `\\host\share` UNC spelling from the
//! `\\?\UNC\` verbatim form (fail-closed policy decision for network paths).

mod absolutize;

use absolutize::absolutize;
use absolutize::absolutize_from;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

/// An absolute path. Construction normalizes platform spellings and expands
/// a leading `~` against the user's home directory.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct AbsolutePathBuf(PathBuf);

/// Strip the Windows verbatim prefix so lexical comparisons behave.
/// Provenance: codex-utils-absolute-path `normalize_path_for_platform`.
///
/// NanoK3 deviation (fail-closed): `\\?\UNC\host\share\...` is the verbatim
/// spelling of the UNC path `\\host\share\...`. Stripping the prefix alone
/// would leave a relative-looking `UNC\...` tail that gets absolutized
/// against the process cwd, deciding a phantom local path while the OS would
/// touch the network share. Reconstruct the canonical UNC spelling so both
/// forms resolve identically.
fn normalize_path_for_platform(path: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if rest
                .get(..4)
                .is_some_and(|head| head.eq_ignore_ascii_case(r"UNC\"))
            {
                // `head` is 4 ASCII bytes, so `rest[4..]` is a char boundary.
                return PathBuf::from(format!(r"\\{}", &rest[4..]));
            }
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}

impl AbsolutePathBuf {
    fn maybe_expand_home_directory(path: &Path) -> PathBuf {
        if let Some(path_str) = path.to_str()
            && let Some(home) = dirs_next::home_dir()
            && let Some(rest) = path_str.strip_prefix('~')
        {
            if rest.is_empty() {
                return home;
            } else if let Some(rest) = rest.strip_prefix('/') {
                return home.join(rest.trim_start_matches('/'));
            } else if cfg!(windows)
                && let Some(rest) = rest.strip_prefix('\\')
            {
                return home.join(rest.trim_start_matches('\\'));
            }
        }
        path.to_path_buf()
    }

    pub fn resolve_path_against_base<P: AsRef<Path>, B: AsRef<Path>>(
        path: P,
        base_path: B,
    ) -> Self {
        let expanded = Self::maybe_expand_home_directory(path.as_ref());
        let expanded = normalize_path_for_platform(&expanded);
        let base_path = normalize_path_for_platform(base_path.as_ref());
        Self(absolutize_from(expanded.as_ref(), base_path.as_ref()))
    }

    /// Expand and absolutize against the process current directory.
    /// (Despite the donor's name, relative input is resolved, not rejected;
    /// use `from_absolute_path_checked` to reject relative input.)
    pub fn from_absolute_path<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let expanded = Self::maybe_expand_home_directory(path.as_ref());
        let expanded = normalize_path_for_platform(&expanded);
        Ok(Self(absolutize(expanded.as_ref())?))
    }

    pub fn from_absolute_path_checked<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let expanded = Self::maybe_expand_home_directory(path.as_ref());
        let expanded = normalize_path_for_platform(&expanded);
        if !expanded.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("path is not absolute: {}", path.as_ref().display()),
            ));
        }
        Ok(Self(absolutize_from(expanded.as_ref(), Path::new("/"))))
    }

    pub fn current_dir() -> std::io::Result<Self> {
        Self::from_absolute_path(std::env::current_dir()?)
    }

    pub fn join<P: AsRef<Path>>(&self, path: P) -> Self {
        Self::resolve_path_against_base(path, &self.0)
    }

    pub fn canonicalize(&self) -> std::io::Result<Self> {
        dunce::canonicalize(&self.0).map(Self)
    }

    pub fn parent(&self) -> Option<Self> {
        self.0.parent().map(|p| {
            debug_assert!(
                p.is_absolute(),
                "parent of AbsolutePathBuf must be absolute"
            );
            Self(p.to_path_buf())
        })
    }

    pub fn ancestors(&self) -> impl Iterator<Item = Self> + '_ {
        self.0.ancestors().map(|p| {
            debug_assert!(
                p.is_absolute(),
                "ancestor of AbsolutePathBuf must be absolute"
            );
            Self(p.to_path_buf())
        })
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.clone()
    }
}

impl<'de> Deserialize<'de> for AbsolutePathBuf {
    /// Paths deserialize by resolving against the process current directory,
    /// matching the donor's guard-less fallback behavior. (The donor's
    /// thread-local base guard is intentionally dropped — Nano resolves
    /// against an explicit cwd at policy evaluation time instead.)
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let path = PathBuf::deserialize(deserializer)?;
        AbsolutePathBuf::from_absolute_path(path).map_err(serde::de::Error::custom)
    }
}

impl std::ops::Deref for AbsolutePathBuf {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&Path> for AbsolutePathBuf {
    /// Resolves against the process current directory (donor parity).
    fn from(path: &Path) -> Self {
        Self::resolve_path_against_base(
            path,
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        )
    }
}

impl From<PathBuf> for AbsolutePathBuf {
    fn from(path: PathBuf) -> Self {
        Self::from(path.as_path())
    }
}

impl AsRef<Path> for AbsolutePathBuf {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl std::fmt::Display for AbsolutePathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.display().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolutizes_against_cwd() {
        let abs = AbsolutePathBuf::from_absolute_path(".").unwrap();
        assert!(abs.as_path().is_absolute());
    }

    #[test]
    fn join_resolves_relative_against_self() {
        let base = AbsolutePathBuf::from_absolute_path(std::env::temp_dir()).unwrap();
        let joined = base.join("child.txt");
        assert!(joined.as_path().is_absolute());
        assert!(joined.as_path().ends_with("child.txt"));
    }

    #[test]
    fn checked_rejects_relative() {
        assert!(AbsolutePathBuf::from_absolute_path_checked("relative/path").is_err());
    }
}
