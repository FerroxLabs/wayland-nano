//! §14 leg 4 — Windows path-canonicalization proof leg: 8.3 short
//! names, forward/backslash separators, and junctions (judged on the
//! resolved target) all collapse to ONE canonical key.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::{
    FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry, FileSystemSandboxPolicy,
    FileSystemSpecialPath,
};
use nano_repomap::{IndexOptions, ReadPolicy, RepoMap};

fn policy(ws: &Path) -> FileSystemSandboxPolicy {
    FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(ws).unwrap(),
            },
            FileSystemAccessMode::Write,
        ),
    ])
}

fn map_for(ws: &Path) -> RepoMap {
    let options = IndexOptions {
        refresh_throttle: Duration::ZERO,
        full_rehash_interval: Duration::ZERO,
        ..Default::default()
    };
    RepoMap::build(ws, options, ReadPolicy::new(&policy(ws), ws)).unwrap()
}

/// The 8.3 short-name spelling of `path` via `GetShortPathNameW`.
/// Returns the long form unchanged when the volume has 8.3 generation
/// disabled (the API then returns the input spelling).
fn short_name(path: &Path) -> PathBuf {
    use std::os::windows::ffi::OsStrExt as _;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // First call sizes the buffer (return = length WITHOUT the nul when
    // the buffer is too small).
    let needed = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
        )
    };
    assert!(needed > 0, "GetShortPathNameW sizing failed for {path:?}");
    let mut buf = vec![0u16; needed as usize + 1];
    let written = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            wide.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
        )
    };
    assert!(written > 0, "GetShortPathNameW failed for {path:?}");
    use std::os::windows::ffi::OsStringExt as _;
    PathBuf::from(std::ffi::OsString::from_wide(&buf[..written as usize]))
}

#[test]
fn short_name_form_maps_to_one_canonical_key() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("workspace with long name");
    std::fs::create_dir_all(&ws).unwrap();
    let file = ws.join("a_very_long_file_name_for_eight_point_three.rs");
    std::fs::write(&file, "pub fn eight_point_three() {}\n").unwrap();
    let map = map_for(&ws);

    let long_entry = map.entry(&file).expect("long form indexed");
    let short = short_name(&file);
    let short_entry = map
        .entry(&short)
        .expect("8.3 short-name form resolves to the same entry");
    assert!(
        std::ptr::eq(long_entry, short_entry),
        "short {short:?} and long forms must collapse to one canonical key"
    );
    if short != file {
        // 8.3 generation is ON for this volume: the collapse above is
        // the substantive proof (two genuinely different spellings).
        assert!(short.to_string_lossy().contains('~'), "short = {short:?}");
    }
    eprintln!(
        "8.3 leg: short={short:?} substantive={} (false = volume has 8.3 disabled, leg vacuous here)",
        short != file
    );
}

#[test]
fn junction_spelling_judged_on_resolved_target() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(ws.join("real")).unwrap();
    std::fs::write(
        ws.join("real/through_junction.rs"),
        "pub fn via_link() {}\n",
    )
    .unwrap();
    // Junction INSIDE the workspace pointing at another in-workspace
    // dir: never traversed by the walker, but a path SPELLED through it
    // canonicalizes to the target — judged on the resolved target.
    let status = std::process::Command::new("cmd")
        .args([
            "/c",
            "mklink",
            "/J",
            &ws.join("linked").to_string_lossy(),
            &ws.join("real").to_string_lossy(),
        ])
        .status()
        .expect("spawn mklink");
    assert!(status.success(), "mklink /J failed: {status}");

    let map = map_for(&ws);
    let direct = map
        .entry(&ws.join("real/through_junction.rs"))
        .expect("indexed");
    let via_junction = map
        .entry(&ws.join("linked/through_junction.rs"))
        .expect("junction spelling resolves to the target's entry");
    assert!(
        std::ptr::eq(direct, via_junction),
        "junction spelling must collapse onto the resolved target's key"
    );
    // And the junction itself added NO duplicate entry: exactly one file.
    assert_eq!(map.stats().files, 1);
}

#[test]
fn backslash_and_forward_slash_spellings_collapse() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("sep.rs"), "pub fn separators() {}\n").unwrap();
    let map = map_for(&ws);
    let back = map.entry(&ws.join("sep.rs")).expect("backslash form");
    let slash = PathBuf::from(ws.join("sep.rs").to_string_lossy().replace('\\', "/"));
    let fwd = map.entry(&slash).expect("forward-slash form");
    assert!(std::ptr::eq(back, fwd));
    assert_eq!(map.stats().files, 1);
}
