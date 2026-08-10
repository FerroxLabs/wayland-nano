//! Adversarial shell tests: policy enforcement must hold — and must fail
//! closed when the sandbox itself cannot be established.
//!
//! The decisive assertion in every escape test is the ABSENCE OF THE SIDE
//! EFFECT (no marker file outside the workspace), not the exit code: a
//! sandbox that silently runs unsandboxed fails these tests and is a hole.

use nano_tools::shell::{ShellKind, ShellTool};
use std::path::PathBuf;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("nano-home");
    let ws = tmp.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    (tmp, home, ws)
}

#[test]
fn write_escape_via_parent_dir_produces_no_side_effect() {
    let (tmp, home, ws) = fixture();
    let escape = tmp.path().join("nanok3-shell-escape.txt");
    let tool = ShellTool::new(&home, &ws);
    // `..` from the sandboxed cwd lands in the tempdir root — outside every
    // writable root of the workspace-write profile.
    let _ = tool.run(
        ShellKind::Cmd,
        "echo pwn > ..\\nanok3-shell-escape.txt",
        Some(std::time::Duration::from_secs(60)),
    );
    assert!(
        !escape.exists(),
        "SECURITY HOLE: sandboxed shell wrote outside the workspace via `..`: {}",
        escape.display()
    );
}

#[test]
fn write_escape_via_absolute_path_produces_no_side_effect() {
    let (tmp, home, ws) = fixture();
    let escape = tmp.path().join("nanok3-shell-escape-abs.txt");
    let tool = ShellTool::new(&home, &ws);
    let command = format!("echo pwn > {}", escape.display());
    let _ = tool.run(
        ShellKind::Cmd,
        &command,
        Some(std::time::Duration::from_secs(60)),
    );
    assert!(
        !escape.exists(),
        "SECURITY HOLE: sandboxed shell wrote outside the workspace via absolute path: {}",
        escape.display()
    );
}

#[test]
fn write_inside_workspace_still_works() {
    // Control: the sandbox is not simply dead — in-policy commands run.
    let (_tmp, home, ws) = fixture();
    let tool = ShellTool::new(&home, &ws);
    let out = tool
        .run(
            ShellKind::Cmd,
            "echo data > nanok3-shell-control.txt",
            Some(std::time::Duration::from_secs(60)),
        )
        .expect("in-policy command must spawn");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(ws.join("nanok3-shell-control.txt").exists());
}

#[test]
fn sandbox_unavailable_fails_closed_with_no_side_effect() {
    // A workspace root that does not exist cannot be sandboxed. The tool must
    // surface a typed spawn error and MUST NOT fall back to running the
    // command unsandboxed.
    let (tmp, home, _ws) = fixture();
    let missing_ws = tmp.path().join("nanok3-missing-workspace");
    let tool = ShellTool::new(&home, &missing_ws);
    let marker_home = home.join("nanok3-must-not-run.txt");
    let marker_tmp = tmp.path().join("nanok3-must-not-run.txt");
    let result = tool.run(
        ShellKind::Cmd,
        "echo ran > nanok3-must-not-run.txt",
        Some(std::time::Duration::from_secs(60)),
    );
    assert!(
        result.is_err(),
        "SECURITY HOLE: command ran even though the sandbox context could not \
         be established: {result:?}"
    );
    assert!(
        !marker_home.exists() && !marker_tmp.exists(),
        "SECURITY HOLE: unsandboxed fallback executed the command"
    );
}
