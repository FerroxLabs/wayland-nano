//! Path normalization helpers.
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/path_normalization.rs`
//! @ 646f7c0a. Transformation: module path only; semantics unchanged.

use std::path::Path;
use std::path::PathBuf;

pub fn canonicalize_path(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Case-folded comparison key on Windows (case-insensitive NTFS); on unix
/// the filesystem is case-sensitive, so the key preserves case — folding
/// there would alias `Foo.rs` and `foo.rs` onto one entry.
pub fn canonical_path_key(path: &Path) -> String {
    let s = canonicalize_path(path).to_string_lossy().replace('\\', "/");
    #[cfg(target_os = "windows")]
    {
        s.to_ascii_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_path_key;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[cfg(target_os = "windows")]
    #[test]
    fn canonical_path_key_normalizes_case_and_separators() {
        let windows_style = Path::new(r"C:\Users\Dev\Repo");
        let slash_style = Path::new("c:/users/dev/repo");

        assert_eq!(
            canonical_path_key(windows_style),
            canonical_path_key(slash_style)
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn canonical_path_key_preserves_case_on_unix() {
        assert_ne!(
            canonical_path_key(Path::new("/tmp/Foo.rs")),
            canonical_path_key(Path::new("/tmp/foo.rs"))
        );
    }
}
