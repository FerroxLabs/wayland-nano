//! gen_error_table — the ONE generator for the C7 error-code artifacts.
//!
//! Everything derives from the Rust table in
//! `nano-session/src/error_codes.rs`; nothing is hand-maintained:
//! - `crates/nano-session/contracts/nano-error-codes.json` (in-repo
//!   canonical copy — the parity test compares against it, so standalone
//!   checkouts and CI work);
//! - `<monorepo>/shared/contracts/nano-error-codes.json` (evidence store of
//!   record — written when the monorepo root is found);
//! - `<monorepo>/desktop/src/common/types/nanoErrorCodes.ts` (+ a JSON copy
//!   for Desktop's parity vitest) when the Desktop checkout is present.
//!
//! `--check` regenerates in memory and exits non-zero on any byte
//! difference (CI tripwire for forgotten regenerations).

use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

/// One generation target: a path plus the bytes that belong there.
struct Target {
    path: PathBuf,
    bytes: String,
    /// The in-repo artifact is mandatory everywhere. The shared mirror is
    /// mandatory whenever a monorepo is detected; Desktop mirrors remain
    /// optional integration outputs.
    required: bool,
}

fn targets_for(repo_root: &Path, monorepo: Option<&Path>) -> Vec<Target> {
    let json = nano_protocol::error_codes::render_json();
    let ts = nano_protocol::error_codes::render_ts();

    let mut targets = vec![Target {
        path: repo_root.join("crates/nano-session/contracts/nano-error-codes.json"),
        bytes: json.clone(),
        required: true,
    }];

    if let Some(root) = monorepo {
        targets.push(Target {
            path: root.join("shared/contracts/nano-error-codes.json"),
            bytes: json.clone(),
            required: true,
        });
        // The Desktop mirror targets NANO_ERROR_TABLE_DESKTOP_DIR when set
        // (a feature-branch worktree of the Desktop repo — Desktop artifacts
        // never land in a checkout that is on another branch); otherwise the
        // sibling checkout at <monorepo>/desktop.
        let desktop_types = std::env::var_os("NANO_ERROR_TABLE_DESKTOP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("desktop"))
            .join("src/common/types");
        if desktop_types.is_dir() {
            targets.push(Target {
                path: desktop_types.join("nanoErrorCodes.ts"),
                bytes: ts,
                required: false,
            });
            targets.push(Target {
                path: desktop_types.join("nano-error-codes.json"),
                bytes: json,
                required: false,
            });
        }
    }
    targets
}

fn targets() -> Vec<Target> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates = manifest.parent().expect("crates dir");
    let repo_root = crates.parent().expect("repo root");

    // The monorepo root is the nearest ancestor carrying shared/reviews.
    let mut dir: Option<&Path> = Some(repo_root);
    let mut monorepo = None;
    while let Some(candidate) = dir {
        if candidate.join("shared/reviews").is_dir() {
            monorepo = Some(candidate);
            break;
        }
        dir = candidate.parent();
    }
    targets_for(repo_root, monorepo)
}

fn check_targets(targets: &[Target]) -> bool {
    let mut failed = false;
    for target in targets {
        match std::fs::read_to_string(&target.path) {
            Ok(existing) if existing == target.bytes => {
                println!("ok: {}", target.path.display());
            }
            Ok(_) => {
                eprintln!("STALE: {} — rerun gen_error_table", target.path.display());
                failed = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && !target.required => {
                println!("skip (absent mirror): {}", target.path.display());
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "MISSING: {} ({err}) — run gen_error_table",
                    target.path.display()
                );
                failed = true;
            }
            Err(err) => {
                eprintln!("UNREADABLE: {} ({err})", target.path.display());
                failed = true;
            }
        }
    }
    !failed
}

fn main() -> ExitCode {
    let check = std::env::args().any(|arg| arg == "--check");
    let targets = targets();
    if check {
        return if check_targets(&targets) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    for target in &targets {
        if let Some(parent) = target.path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact dir");
        }
        std::fs::write(&target.path, &target.bytes).expect("write artifact");
        println!("wrote: {}", target.path.display());
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_shared_target_is_required_and_missing_fails_check() {
        let temp = tempfile::tempdir().expect("create isolated monorepo");
        let repo_root = temp.path().join("wayland-nano");
        let configured = targets_for(&repo_root, Some(temp.path()));
        let shared = configured
            .into_iter()
            .find(|target| {
                target
                    .path
                    .ends_with("shared/contracts/nano-error-codes.json")
            })
            .expect("detected monorepo configures canonical shared target");
        let missing = temp.path().join("shared/contracts/nano-error-codes.json");
        assert_eq!(shared.path, missing);
        assert!(shared.required, "canonical shared target must fail closed");
        assert!(!check_targets(&[shared]));
        assert!(!missing.exists(), "check mode must not create the mirror");
    }
}
