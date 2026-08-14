//! Sensitive-path predicate — the lexical "looks like a credential store"
//! check shared by every read surface (fs tools, search/glob, the P4
//! repomap).
//!
//! Provenance: moved verbatim from `nano-tools/src/fs.rs` (which re-exports
//! it) so `nano-repomap` can compose the SAME predicate without a circular
//! dependency (P4 design §5.3: one definition, one seam). Pure lexical
//! check — no IO, no OS knowledge beyond path spelling.

use std::path::Path;

const SENSITIVE_BASENAMES: &[&str] = &[".env", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];
const SENSITIVE_EXTENSIONS: &[&str] = &[".pem", ".key", ".pfx", ".p12", ".kdbx"];

pub fn is_sensitive_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if SENSITIVE_BASENAMES
        .iter()
        .any(|b| name.eq_ignore_ascii_case(b))
    {
        return true;
    }
    // Case-insensitive to match the basename equality checks above: on
    // case-insensitive filesystems (Windows, the primary platform)
    // `.ENV.PRODUCTION` names the same credential store as `.env.production`.
    let lower = name.to_ascii_lowercase();
    if lower.starts_with(".env.") {
        return true;
    }
    SENSITIVE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn sensitive_path_detection() {
        assert!(is_sensitive_path(Path::new("/repo/.env")));
        assert!(is_sensitive_path(Path::new("/repo/.env.production")));
        assert!(is_sensitive_path(Path::new("/home/u/.ssh/id_rsa")));
        assert!(is_sensitive_path(Path::new("/certs/server.pem")));
        assert!(!is_sensitive_path(Path::new("/repo/notes.txt")));
        assert!(!is_sensitive_path(Path::new("/repo/environment.rs")));
    }

    #[test]
    fn env_prefix_is_case_insensitive() {
        // F-28 LOW-8: the `.env.` prefix check must match the case-insensitive
        // basename equality — `.ENV.PRODUCTION` is the same file as
        // `.env.production` on case-insensitive filesystems (Windows).
        assert!(is_sensitive_path(Path::new("/repo/.ENV.PRODUCTION")));
        assert!(is_sensitive_path(Path::new("/repo/.Env.Production")));
        assert!(is_sensitive_path(Path::new("/repo/.env.LOCAL")));
        assert!(is_sensitive_path(Path::new("C:\\repo\\.EnV.Staging")));
        // Tightening only: near-misses that were never caught stay clear.
        assert!(!is_sensitive_path(Path::new("/repo/.ENVRC")));
        assert!(!is_sensitive_path(Path::new("/repo/.ENVIRONMENT")));
        assert!(!is_sensitive_path(Path::new("/repo/env.production")));
        // Basename equality arm was already case-insensitive.
        assert!(is_sensitive_path(Path::new("/repo/.ENV")));
    }
}
