//! Flux API key resolution shared by all host modes.
//!
//! Order: `FLUX_API_KEY`, then `FLUX_TEST_KEY`, then the file named by
//! `FLUX_API_KEY_FILE`. The file fallback exists so host-spawners (Desktop's
//! ACP launcher, scripts) can pass a *path* instead of the secret itself —
//! the key then lives in exactly one file, never in config blobs, command
//! lines, or process env dumps.

pub fn flux_api_key() -> Option<String> {
    for var in ["FLUX_API_KEY", "FLUX_TEST_KEY"] {
        if let Ok(key) = std::env::var(var) {
            let key = key.trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }
    let path = std::env::var("FLUX_API_KEY_FILE").ok()?;
    let contents = std::fs::read_to_string(path.trim()).ok()?;
    let key = contents.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
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

        // File fallback works and trims whitespace/newlines.
        let mut path = std::env::temp_dir();
        path.push(format!("nanok3-flux-key-test-{}.txt", std::process::id()));
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "  sk-from-file  ").unwrap();
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
        unsafe { std::env::set_var("FLUX_API_KEY_FILE", "D:/nonexistent/nanok3-no-key.txt") };
        assert_eq!(flux_api_key(), None);

        unsafe { std::env::remove_var("FLUX_API_KEY_FILE") };
    }
}
