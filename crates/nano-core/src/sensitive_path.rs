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
    if name.starts_with(".env.") {
        return true;
    }
    let lower = name.to_ascii_lowercase();
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
}
