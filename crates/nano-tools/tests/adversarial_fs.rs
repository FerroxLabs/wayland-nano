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
    std::fs::write(&env_outside, "NANO_SECRET=1").unwrap();
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
    let junction = ws.join("nano-junction");
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
    let escape = outside.join("nano-junction-escape.txt");
    let err = tools.write_file(&junction.join("nano-junction-escape.txt"), "pwn");
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
    let junction = ws.join("nano-junction");
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

// --- Hard-link escapes ---------------------------------------------------------
//
// A hard link is a second NAME for the same file object — no reparse point —
// so canonicalization cannot see that an in-workspace name aliases an object
// outside the writable root. `fs::hard_link` needs no special privilege on
// Windows. The policy engine denies writes to any existing file with more
// than one link; the OS DACL layer binds the access check to the file OBJECT
// (every name of it), which is what contains the sandbox identity. Evidence
// and analysis: `docs/audits/hardlink-race.md`.

#[test]
fn write_through_hard_link_is_denied_and_does_not_land_outside() {
    let (_tmp, ws, outside) = fixture();
    let target = outside.join("nano-secret.txt");
    let link = ws.join("nano-hardlink.txt");
    std::fs::hard_link(&target, &link).expect("hard link needs no privilege");

    let tools = FsTools::new(workspace_policy(&ws), &ws);
    let err = tools.write_file(&link, "pwn");
    assert!(
        matches!(err, Err(ToolError::WriteDenied(_))),
        "SECURITY HOLE: write through a hard link inside the workspace was not \
         denied: {err:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "nano-outside-secret",
        "SECURITY HOLE: write through a hard link mutated the out-of-root object"
    );
}

#[test]
fn read_through_hard_link_into_denied_dir_documents_alias_transparency() {
    // PLATFORM LIMITATION (docs/audits/hardlink-race.md): the lexical policy
    // layer cannot distinguish a hard link's names, so the read through an
    // in-workspace alias of a deny-read file is NOT denied at this layer.
    // Containment comes from the OS: the sandbox identity has no read ACE on
    // the target object (proven by
    // `target_dacl_denies_read_through_preplanted_hard_link` below), so the
    // same attempt inside the sandbox fails at the DACL.
    let (_tmp, ws, outside) = fixture();
    let target = outside.join("nano-secret.txt");
    let link = ws.join("nano-hardlink.txt");
    std::fs::hard_link(&target, &link).expect("hard link needs no privilege");

    let tools = FsTools::new(deny_read_policy(&outside), &ws);
    let (content, _) = tools
        .read_file(&link, &ReadBounds::default())
        .expect("documented limitation: policy layer is alias-transparent for hard links");
    assert_eq!(content, "nano-outside-secret");
}

/// `DOMAIN\user` for icacls ACEs.
#[cfg(windows)]
fn whoami() -> String {
    let output = std::process::Command::new("whoami")
        .output()
        .expect("spawn whoami");
    assert!(output.status.success(), "whoami failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Runs icacls, asserting success. Used to place/remove deny ACEs emulating
/// the sandbox identity's missing grant on an outside object: an absent allow
/// ACE denies exactly like an explicit deny ACE.
#[cfg(windows)]
fn icacls(args: &[String]) {
    let output = std::process::Command::new("icacls")
        .args(args)
        .output()
        .expect("spawn icacls");
    assert!(
        output.status.success(),
        "icacls {args:?} failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(windows)]
#[test]
fn target_dacl_denies_write_through_preplanted_hard_link() {
    // OS-layer containment proof: even with the alias PRE-PLANTED inside the
    // workspace (the ambient same-user race), the write is access-checked
    // against the TARGET file object's DACL, not the directory entry. A raw
    // `std::fs::write` bypasses the policy check on purpose: the DACL alone
    // must stop the write.
    let (_tmp, ws, outside) = fixture();
    let target = outside.join("nano-secret.txt");
    let link = ws.join("nano-hardlink.txt");
    std::fs::hard_link(&target, &link).expect("hard link needs no privilege");

    let me = whoami();
    let target_arg = target.display().to_string();
    icacls(&[target_arg.clone(), "/deny".into(), format!("{me}:(W)")]);

    let err = std::fs::write(&link, "pwn")
        .expect_err("target DACL must deny the write through the alias");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    icacls(&[target_arg, "/remove:d".into(), me]);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "nano-outside-secret",
        "outside object mutated despite the DACL deny"
    );
}

#[cfg(windows)]
#[test]
fn hard_link_creation_to_write_denied_target_fails() {
    // Self-limiting creation: since Windows 10, CreateHardLink requires write
    // access to the TARGET. The sandbox identity has no write ACE on outside
    // objects, so it cannot even plant the alias itself.
    //
    // Environment note: this holds for NON-ADMIN identities. Administrators
    // (e.g. hosted CI runners) can create the link despite the deny ACE —
    // SeRestorePrivilege bypasses the target check. In that case the
    // containment that matters is the POLICY layer: the engine denies writes
    // to multi-link files outright (policy_engine existing_file_has_multiple_links),
    // so a planted alias is unwritable through the tools either way.
    let (_tmp, ws, outside) = fixture();
    let target = outside.join("nano-secret.txt");
    let me = whoami();
    let target_arg = target.display().to_string();
    icacls(&[target_arg.clone(), "/deny".into(), format!("{me}:(W)")]);

    let link = ws.join("nano-hardlink.txt");
    match std::fs::hard_link(&target, &link) {
        Err(err) => {
            // Non-admin path: creation itself is refused.
            assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(!link.exists());
        }
        Ok(()) => {
            // Admin-context path (CI runners): creation succeeds, so the
            // write-through must be denied at the policy layer instead.
            // Deterministic on every host: the multi-link probe is fail-closed
            // (an existing file that cannot be opened/probed denies the write),
            // which also covers runners where DACL evaluation blocks the probe.
            // NOTE: no content read of `target` here — the `(W)` deny ACE also
            // strips READ_CONTROL (STANDARD_RIGHTS_WRITE), so GENERIC_READ
            // opens fail on elevated runners; the policy denial is the oracle.
            let policy = workspace_policy(&ws);
            assert!(
                !policy.can_write_path_with_cwd(&link, &ws),
                "admin-planted hard link must be write-denied at the policy layer"
            );
        }
    }

    icacls(&[target_arg, "/remove:d".into(), me]);
}

#[cfg(windows)]
#[test]
fn target_dacl_denies_read_through_preplanted_hard_link() {
    // The read-side counterpart: the target object's DACL binds every name,
    // so a denied target stays unreadable through an in-workspace alias. This
    // is the containment for the policy layer's read alias transparency.
    let (_tmp, ws, outside) = fixture();
    let target = outside.join("nano-secret.txt");
    let link = ws.join("nano-hardlink.txt");
    std::fs::hard_link(&target, &link).expect("hard link needs no privilege");

    let me = whoami();
    let target_arg = target.display().to_string();
    icacls(&[target_arg.clone(), "/deny".into(), format!("{me}:(R)")]);

    let err = std::fs::read(&link).expect_err("target DACL must deny the read through the alias");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    icacls(&[target_arg, "/remove:d".into(), me]);
}
