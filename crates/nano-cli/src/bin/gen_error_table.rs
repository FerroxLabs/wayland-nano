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
    /// Monorepo mirrors are optional in standalone checkouts; the in-repo
    /// artifact is mandatory everywhere.
    required: bool,
}

fn targets() -> Vec<Target> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates = manifest.parent().expect("crates dir");
    let repo_root = crates.parent().expect("repo root");
    let json = nano_protocol::error_codes::render_json();
    let ts = nano_protocol::error_codes::render_ts();

    let mut targets = vec![Target {
        path: repo_root.join("crates/nano-session/contracts/nano-error-codes.json"),
        bytes: json.clone(),
        required: true,
    }];

    // The monorepo root is the nearest ancestor carrying shared/reviews.
    let mut dir: Option<&Path> = Some(repo_root);
    let mut monorepo = None;
    while let Some(candidate) = dir {
        if candidate.join("shared/reviews").is_dir() {
            monorepo = Some(candidate.to_path_buf());
            break;
        }
        dir = candidate.parent();
    }
    if let Some(root) = monorepo {
        targets.push(Target {
            path: root.join("shared/contracts/nano-error-codes.json"),
            bytes: json.clone(),
            required: false,
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

fn main() -> ExitCode {
    let check = std::env::args().any(|arg| arg == "--check");
    let targets = targets();
    let mut failed = false;
    for target in &targets {
        if check {
            match std::fs::read_to_string(&target.path) {
                Ok(existing) if existing == target.bytes => {
                    println!("ok: {}", target.path.display());
                }
                Ok(_) => {
                    eprintln!("STALE: {} — rerun gen_error_table", target.path.display());
                    failed = true;
                }
                Err(err) => {
                    if target.required {
                        eprintln!(
                            "MISSING: {} ({err}) — run gen_error_table",
                            target.path.display()
                        );
                        failed = true;
                    } else {
                        println!("skip (absent mirror): {}", target.path.display());
                    }
                }
            }
        } else {
            if let Some(parent) = target.path.parent() {
                std::fs::create_dir_all(parent).expect("create artifact dir");
            }
            std::fs::write(&target.path, &target.bytes).expect("write artifact");
            println!("wrote: {}", target.path.display());
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
