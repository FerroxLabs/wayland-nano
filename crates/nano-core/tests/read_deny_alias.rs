//! Alias-spelling read-deny tests: a deny root must hold for queries spelled
//! through an alias (symlink, NTFS junction, 8.3 short name, macOS
//! `/var` -> `/private/var`) even when the queried file does NOT exist yet.
//!
//! Regression cover for CI run 31685142953:
//! `nano_repomap::policy::tests::denied_read_path_never_allowed` failed on
//! windows-latest / windows-11-arm / macos-15-intel / macos-14 because the
//! runner tempdir is an alias of its canonical path: the deny root is stored
//! canonicalized, while a query for a not-yet-created file could not be
//! canonicalized and kept only the alias spelling — which never prefix-matches
//! the canonical deny root. The matcher now canonicalizes the deepest
//! EXISTING ancestor of a missing target and re-appends the lexical
//! remainder, symmetrically for query paths and deny entries.
//!
//! The repomap walker's never-follow-symlinks rule is a separate, structural
//! layer; these tests attack the matcher directly.

use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::FileSystemAccessMode;
use nano_core::permissions::FileSystemPath;
use nano_core::permissions::FileSystemSandboxEntry;
use nano_core::permissions::FileSystemSandboxPolicy;
use nano_core::permissions::FileSystemSpecialPath;
use nano_core::policy_engine::ReadDenyMatcher;
use std::path::Path;

/// Root read + explicit deny on `denied`.
fn policy(denied: &Path) -> FileSystemSandboxPolicy {
    FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(denied).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ),
    ])
}

fn matcher(policy: &FileSystemSandboxPolicy, cwd: &Path) -> ReadDenyMatcher {
    ReadDenyMatcher::new(policy, cwd).expect("policy has deny-read restrictions")
}

/// Asserts the alias-spelling contract for a directory link `alias` -> `real`
/// where `real/secret` is denied: existing and not-yet-created files queried
/// through the alias spelling are denied, while non-denied siblings under the
/// same alias stay allowed (no over-denial from prefix sloppiness).
fn assert_alias_contract(alias: &Path, real: &Path) {
    let denied = real.join("secret");
    std::fs::create_dir_all(&denied).unwrap();
    std::fs::write(denied.join("existing.rs"), "nano-alias-secret").unwrap();
    let public = real.join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("existing.rs"), "nano-alias-public").unwrap();

    let policy = policy(&denied);
    let matcher = matcher(&policy, real);
    assert!(
        matcher.is_read_denied(&denied.join("existing.rs")),
        "control: canonical spelling of an existing denied file"
    );

    assert!(
        matcher.is_read_denied(&alias.join("secret/existing.rs")),
        "existing file spelled through the alias must be denied"
    );
    assert!(
        matcher.is_read_denied(&alias.join("secret/future.rs")),
        "not-yet-created file spelled through the alias must be denied"
    );
    assert!(
        !matcher.is_read_denied(&alias.join("public/existing.rs")),
        "non-denied sibling (existing) through the alias must stay allowed"
    );
    assert!(
        !matcher.is_read_denied(&alias.join("public/future.rs")),
        "non-denied sibling (not yet created) through the alias must stay allowed"
    );
}

#[cfg(unix)]
#[test]
fn symlink_alias_spelling_stays_denied_for_missing_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let alias = tmp.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();

    assert_alias_contract(&alias, &real);
}

#[cfg(windows)]
#[test]
fn junction_alias_spelling_stays_denied_for_missing_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let alias = tmp.path().join("alias");
    // Directory junctions need no elevation (unlike symlinks).
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&alias)
        .arg(&real)
        .output()
        .expect("spawn mklink");
    assert!(status.status.success(), "mklink /J failed");

    assert_alias_contract(&alias, &real);
}

/// Deny entries pointing at not-yet-created paths get the same
/// ancestor-canonicalization treatment as query paths: a deny SPELLED THROUGH
/// the alias must hold under the canonical spelling, and vice versa.
#[cfg(unix)]
#[test]
fn deny_entry_on_missing_path_matches_under_both_spellings() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let alias = tmp.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();

    // `pending` does NOT exist on disk; the deny entry is alias-spelled.
    let pending_via_alias = alias.join("pending");
    let policy = policy(&pending_via_alias);
    let matcher = matcher(&policy, &real);

    assert!(
        matcher.is_read_denied(&real.join("pending/future.rs")),
        "alias-spelled deny on a missing path must hold under the canonical spelling"
    );
    assert!(
        matcher.is_read_denied(&alias.join("pending/future.rs")),
        "alias-spelled deny on a missing path must hold under the alias spelling"
    );
    assert!(
        !matcher.is_read_denied(&real.join("other/future.rs")),
        "non-denied sibling must stay allowed"
    );
}
