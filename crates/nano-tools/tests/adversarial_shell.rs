//! Adversarial shell tests: policy enforcement must hold — and must fail
//! closed when the sandbox itself cannot be established.
//!
//! The decisive assertion in every escape test is the ABSENCE OF THE SIDE
//! EFFECT (no marker file outside the workspace), not the exit code: a
//! sandbox that silently runs unsandboxed fails these tests and is a hole.

use nano_tools::shell::ShellKind;
use nano_tools::shell::ShellTool;
use std::path::PathBuf;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("nano-home");
    let ws = tmp.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    (tmp, home, ws)
}

/// Per-test TEMP/TMP so the sandbox capture ACLs a per-test scoped temp
/// root instead of the shared one (parallel cargo test races the
/// fail-closed ACL verification otherwise — this file's escape tests
/// proved the flake: `write_inside_workspace_still_works` intermittently
/// failed under `--workspace` parallelism).
fn fixture_env(tmp: &tempfile::TempDir) -> std::collections::HashMap<String, String> {
    let p = tmp.path().to_string_lossy().into_owned();
    std::collections::HashMap::from([("TEMP".to_string(), p.clone()), ("TMP".to_string(), p)])
}

#[cfg(windows)]
#[test]
fn write_escape_via_parent_dir_produces_no_side_effect() {
    let (tmp, home, ws) = fixture();
    let escape = tmp.path().join("nano-shell-escape.txt");
    let tool = ShellTool::new(&home, &ws);
    // `..` from the sandboxed cwd lands in the tempdir root — outside every
    // writable root of the workspace-write profile.
    let _ = tool.run_with_env(
        ShellKind::Cmd,
        "echo pwn > ..\\nano-shell-escape.txt",
        Some(std::time::Duration::from_secs(60)),
        fixture_env(&tmp),
    );
    assert!(
        !escape.exists(),
        "SECURITY HOLE: sandboxed shell wrote outside the workspace via `..`: {}",
        escape.display()
    );
}

#[cfg(windows)]
#[test]
fn write_escape_via_absolute_path_produces_no_side_effect() {
    let (tmp, home, ws) = fixture();
    let escape = tmp.path().join("nano-shell-escape-abs.txt");
    let tool = ShellTool::new(&home, &ws);
    let command = format!("echo pwn > {}", escape.display());
    let _ = tool.run_with_env(
        ShellKind::Cmd,
        &command,
        Some(std::time::Duration::from_secs(60)),
        fixture_env(&tmp),
    );
    assert!(
        !escape.exists(),
        "SECURITY HOLE: sandboxed shell wrote outside the workspace via absolute path: {}",
        escape.display()
    );
}

#[cfg(windows)]
#[test]
fn write_inside_workspace_still_works() {
    // Control: the sandbox is not simply dead — in-policy commands run.
    let (_tmp, home, ws) = fixture();
    let tool = ShellTool::new(&home, &ws);
    let out = tool
        .run_with_env(
            ShellKind::Cmd,
            "echo data > nano-shell-control.txt",
            Some(std::time::Duration::from_secs(60)),
            fixture_env(&_tmp),
        )
        .expect("in-policy command must spawn");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(ws.join("nano-shell-control.txt").exists());
}

#[cfg(windows)]
#[test]
fn sandbox_unavailable_fails_closed_with_no_side_effect() {
    // A workspace root that does not exist cannot be sandboxed. The tool must
    // surface a typed spawn error and MUST NOT fall back to running the
    // command unsandboxed.
    let (tmp, home, _ws) = fixture();
    let missing_ws = tmp.path().join("nano-missing-workspace");
    let tool = ShellTool::new(&home, &missing_ws);
    let marker_home = home.join("nano-must-not-run.txt");
    let marker_tmp = tmp.path().join("nano-must-not-run.txt");
    let result = tool.run_with_env(
        ShellKind::Cmd,
        "echo ran > nano-must-not-run.txt",
        Some(std::time::Duration::from_secs(60)),
        fixture_env(&tmp),
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

// --- Unix legs (run for real on the CI unix runners) ----------------------
// The Linux leg needs the `wayland-nano-linux-sandbox` helper; `cargo test
// --workspace` builds it into `target/debug/`, which the shell tool's helper
// resolution finds from the `deps/` test executable directory.

#[cfg(unix)]
#[test]
fn unix_write_escape_via_parent_dir_produces_no_side_effect() {
    let (tmp, home, ws) = fixture();
    let escape = tmp.path().join("nano-shell-escape.txt");
    let tool = ShellTool::new(&home, &ws);
    // `..` from the sandboxed cwd lands in the tempdir root — outside every
    // writable root of the workspace-write profile.
    let _ = tool.run_with_env(
        ShellKind::Sh,
        "echo pwn > ../nano-shell-escape.txt",
        Some(std::time::Duration::from_secs(60)),
        fixture_env(&tmp),
    );
    assert!(
        !escape.exists(),
        "SECURITY HOLE: sandboxed shell wrote outside the workspace via `..`: {}",
        escape.display()
    );
}

#[cfg(unix)]
#[test]
fn unix_write_escape_via_absolute_path_produces_no_side_effect() {
    let (tmp, home, ws) = fixture();
    let escape = tmp.path().join("nano-shell-escape-abs.txt");
    let tool = ShellTool::new(&home, &ws);
    let command = format!("echo pwn > {}", escape.display());
    let _ = tool.run_with_env(
        ShellKind::Sh,
        &command,
        Some(std::time::Duration::from_secs(60)),
        fixture_env(&tmp),
    );
    assert!(
        !escape.exists(),
        "SECURITY HOLE: sandboxed shell wrote outside the workspace via absolute path: {}",
        escape.display()
    );
}

#[cfg(unix)]
#[test]
fn unix_write_inside_workspace_still_works() {
    // Control: the sandbox is not simply dead — in-policy commands run.
    let (_tmp, home, ws) = fixture();
    let tool = ShellTool::new(&home, &ws);
    let out = tool
        .run_with_env(
            ShellKind::Sh,
            "echo data > nano-shell-control.txt",
            Some(std::time::Duration::from_secs(60)),
            fixture_env(&_tmp),
        )
        .expect("in-policy command must spawn");
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(ws.join("nano-shell-control.txt").exists());
}

#[cfg(unix)]
#[test]
fn unix_sandbox_unavailable_fails_closed_with_no_side_effect() {
    // A workspace root that does not exist cannot be spawned into. The tool
    // must surface a typed error and MUST NOT fall back to running the
    // command unsandboxed.
    let (tmp, home, _ws) = fixture();
    let missing_ws = tmp.path().join("nano-missing-workspace");
    let tool = ShellTool::new(&home, &missing_ws);
    let marker_home = home.join("nano-must-not-run.txt");
    let marker_tmp = tmp.path().join("nano-must-not-run.txt");
    let result = tool.run_with_env(
        ShellKind::Sh,
        "echo ran > nano-must-not-run.txt",
        Some(std::time::Duration::from_secs(60)),
        fixture_env(&tmp),
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
