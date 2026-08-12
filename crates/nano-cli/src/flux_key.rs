//! Flux API key resolution shared by all host modes.
//!
//! Order: `FLUX_API_KEY`, then `FLUX_TEST_KEY`, then the file named by
//! `FLUX_API_KEY_FILE`. The file fallback exists so host-spawners (Desktop's
//! ACP launcher, scripts) can pass a *path* instead of the secret itself —
//! the key then lives in exactly one file, never in config blobs, command
//! lines, or process env dumps. C8: the file read goes through the shared
//! perms-gated `read_key_file` (0600 on unix; Desktop writes it that way).

pub fn flux_api_key() -> Option<String> {
    crate::provider_key::resolve_flux(&|name| std::env::var(name).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // Env-mutating tests must not run in parallel with each other; keep them
    // in one test body so the harness serializes them.
    #[test]
    fn resolution_order_and_file_fallback() {
        // SAFETY: single-threaded within this test; no other test in this
        // crate touches these vars.
        unsafe {
            std::env::remove_var("FLUX_API_KEY");
            std::env::remove_var("FLUX_TEST_KEY");
            std::env::remove_var("FLUX_API_KEY_FILE");
        }
        assert_eq!(flux_api_key(), None);

        // File fallback works and trims whitespace/newlines. C8: key files
        // must be owner-only on unix (the perms gate) — chmod before use.
        let mut path = std::env::temp_dir();
        path.push(format!("nano-flux-key-test-{}.txt", std::process::id()));
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "  sk-from-file  ").unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        unsafe { std::env::set_var("FLUX_API_KEY_FILE", &path) };
        assert_eq!(flux_api_key().as_deref(), Some("sk-from-file"));

        // Env wins over the file.
        unsafe { std::env::set_var("FLUX_API_KEY", "sk-from-env") };
        assert_eq!(flux_api_key().as_deref(), Some("sk-from-env"));

        // Empty file yields None.
        std::fs::write(&path, "   \n").unwrap();
        unsafe { std::env::remove_var("FLUX_API_KEY") };
        assert_eq!(flux_api_key(), None);

        // Missing file yields None.
        unsafe { std::env::set_var("FLUX_API_KEY_FILE", "D:/nonexistent/nano-no-key.txt") };
        assert_eq!(flux_api_key(), None);

        unsafe { std::env::remove_var("FLUX_API_KEY_FILE") };
    }
}
