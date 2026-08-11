//! Adversarial filesystem tests: path traversal and link-escape attempts
//! against the direct-tool policy checks in `nano_tools::fs`.
//!
//! Fixture style mirrors `fs.rs` unit tests: a tempdir holding a `workspace`
//! root (writable) plus an `outside` directory (never writable, deny-read
//! where reads are under test). All marker/secret files are nano-namespaced.

use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::{
    FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry, FileSystemSandboxPolicy,
    FileSystemSpecialPath,
};
use nano_tools::fs::{FsTools, ReadBounds, ToolError};
use std::path::{Path, PathBuf};

/// Root read + workspace write: the standard agent fs posture.
fn workspace_policy(workspace: &Path) -> FileSystemSandboxPolicy {
    FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(workspace).unwrap(),
            },
            FileSystemAccessMode::Write,
        ),
    ])
}

/// Root read with an explicit deny-read entry on `denied_dir`.
fn deny_read_policy(denied_dir: &Path) -> FileSystemSandboxPolicy {
    FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(denied_dir).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ),
    ])
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("workspace");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("nano-secret.txt"), "nano-outside-secret").unwrap();
    (tmp, ws, outside)
}

/// Create a directory link `link` -> `target`: NTFS junction on Windows
/// (no privilege required), symlink elsewhere. Returns false when the
/// platform refused (e.g. Windows symlink without developer mode/admin).
fn make_dir_link(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        // Junctions work without elevation and are resolved by the OS like
        // symlinks for path purposes.
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("spawn mklink");
        return status.status.success();
    }
    #[cfg(unix)]
    {
        return std::os::unix::fs::symlink(target, link).is_ok();
    }
    #[allow(unreachable_code)]
    false
}

// --- ../ traversal -----------------------------------------------------------

#[test]
fn write_through_dotdot_chain_is_denied() {
    let (tmp, ws, _outside) = fixture();
    let tools = FsTools::new(workspace_policy(&ws), &ws);
    let escape = tmp.path().join("nano-traversal.txt");

    let attempts = [
        ws.join("../nano-traversal.txt"),
        ws.join("sub/../../nano-traversal.txt"),
        ws.join("../../nano-traversal.txt"),
        // absolute path smuggling a `..` past naive prefix checks
        tmp.path().join("workspace/../nano-traversal.txt"),
    ];
    for attempt in attempts {
        let err = tools.write_file(&attempt, "pwn").expect_err("must deny");
        assert!(
            matches!(err, ToolError::WriteDenied(_)),
            "{} denied with wrong variant: {err:?}",
            attempt.display()
        );
        assert!(
            !escape.exists(),
            "{} escaped the workspace",
            attempt.display()
        );
    }
}

#[test]
fn dotdot_that_stays_inside_workspace_is_allowed() {
    // Control: normalization must not over-deny legitimate in-root paths.
    let (_tmp, ws, _outside) = fixture();
    std::fs::create_dir_all(ws.join("sub")).unwrap();
    let tools = FsTools::new(workspace_policy(&ws), &ws);
    let target = ws.join("sub/../nano-ok.txt");
    tools.write_file(&target, "fine").expect("in-root write");
    assert!(ws.join("nano-ok.txt").exists());
}

#[test]
fn read_through_dotdot_into_denied_dir_is_denied() {
    let (_tmp, ws, outside) = fixture();
    let tools = FsTools::new(deny_read_policy(&outside), &ws);
    let attempts = [
        ws.join("../outside/nano-secret.txt"),
        outside.join("nano-secret.txt"),
    ];
    for attempt in attempts {
        let err = tools
            .read_file(&attempt, &ReadBounds::default())
            .expect_err("must deny");
        assert!(
            matches!(err, ToolError::ReadDenied(_)),
            "{} denied with wrong variant: {err:?}",
            attempt.display()
        );
    }
}

#[test]
fn sensitive_file_outside_workspace_denied_by_name_via_traversal() {
    let (tmp, ws, _outside) = fixture();
    let env_outside = tmp.path().join(".env");
    std::fs::write(&env_outside, "NANOK3_SECRET=1").unwrap();
    let tools = FsTools::new(workspace_policy(&ws), &ws);
    let err = tools
        .read_file(&ws.join("../.env"), &ReadBounds::default())
        .expect_err("must deny");
    assert!(matches!(err, ToolError::SensitiveDenied(_)));
}

// --- Symlink / junction escapes ----------------------------------------------

#[test]
fn read_through_dir_link_into_denied_dir_is_denied() {
    let (_tmp, ws, outside) = fixture();
    let link = ws.join("nano-link");
    if !make_dir_link(&link, &outside) {
        eprintln!("skipping: directory link creation not permitted on this host");
        return;
    }
    let tools = FsTools::new(deny_read_policy(&outside), &ws);
    let err = tools
        .read_file(&link.join("nano-secret.txt"), &ReadBounds::default())
        .expect_err("read through link into denied dir must be denied");
    assert!(
        matches!(err, ToolError::ReadDenied(_)),
        "denied with wrong variant: {err:?}"
    );
}

#[test]
fn write_through_dir_link_escapes_workspace_must_be_denied() {
    let (_tmp, ws, outside) = fixture();
    let link = ws.join("nano-link");
    if !make_dir_link(&link, &outside) {
        eprintln!("skipping: directory link creation not permitted on this host");
        return;
    }
    let tools = FsTools::new(workspace_policy(&ws), &ws);
    let escape = outside.join("nano-link-escape.txt");
    let err = tools.write_file(&link.join("nano-link-escape.txt"), "pwn");
    assert!(
        matches!(err, Err(ToolError::WriteDenied(_))),
        "SECURITY HOLE: write through a directory link inside the workspace \
         landed outside the root: {err:?}"
    );
    assert!(
        !escape.exists(),
        "SECURITY HOLE: {} was created through a link escape",
        escape.display()
    );
}

#[cfg(windows)]
#[test]
fn ntfs_junction_write_escape_must_be_denied() {
    // Junctions need no privilege and historically bypass lexical checks that
    // only special-case symlinks — exercise them explicitly.
    let (_tmp, ws, outside) = fixture();
    let junction = ws.join("nanok3-junction");
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .expect("spawn mklink");
    assert!(
        status.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let tools = FsTools::new(workspace_policy(&ws), &ws);
    let escape = outside.join("nanok3-junction-escape.txt");
    let err = tools.write_file(&junction.join("nanok3-junction-escape.txt"), "pwn");
    assert!(
        matches!(err, Err(ToolError::WriteDenied(_))),
        "SECURITY HOLE: write through an NTFS junction escaped the workspace: {err:?}"
    );
    assert!(
        !escape.exists(),
        "SECURITY HOLE: {} was created through a junction escape",
        escape.display()
    );
}

#[cfg(windows)]
#[test]
fn ntfs_junction_read_into_denied_dir_is_denied() {
    let (_tmp, ws, outside) = fixture();
    let junction = ws.join("nanok3-junction");
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .expect("spawn mklink");
    assert!(status.status.success(), "mklink /J failed");
    let tools = FsTools::new(deny_read_policy(&outside), &ws);
    let err = tools
        .read_file(&junction.join("nano-secret.txt"), &ReadBounds::default())
        .expect_err("read through junction into denied dir must be denied");
    assert!(
        matches!(err, ToolError::ReadDenied(_)),
        "denied with wrong variant: {err:?}"
    );
}
