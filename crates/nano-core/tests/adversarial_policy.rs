//! Adversarial policy tests (round 2): Windows path-spelling confusion against
//! the nano-core filesystem policy engine.
//!
//! The enforcement boundary under test is the one `nano_tools::fs` uses:
//! reads go through `ReadDenyMatcher::is_read_denied`, writes through
//! `FileSystemSandboxPolicy::can_write_path_with_cwd`. Every spelling variant
//! of a target must resolve to the SAME decision as its canonical form — and
//! must NEVER be more permissive (no allow-by-spelling-variant). Variants may
//! be STRICTER than canonical (fail-closed); that is fine and noted.
//!
//! A failing assertion named SECURITY HOLE means a spelling variant bypasses
//! a deny. Those tests are intentionally left failing until the engine is
//! fixed — do not weaken them.

#![cfg(windows)]

use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::FileSystemAccessMode;
use nano_core::permissions::FileSystemPath;
use nano_core::permissions::FileSystemSandboxEntry;
use nano_core::permissions::FileSystemSandboxPolicy;
use nano_core::permissions::FileSystemSpecialPath;
use nano_core::policy_engine::ReadDenyMatcher;
use nano_core::policy_engine::workspace_write_policy;
use std::path::Path;
use std::path::PathBuf;

/// Permissiveness ladder: Deny < Read < Write.
fn rank(mode: FileSystemAccessMode) -> u8 {
    u8::from(mode.can_read()) + u8::from(mode.can_write())
}

fn assert_not_more_permissive(
    canonical: FileSystemAccessMode,
    variant: FileSystemAccessMode,
    label: &str,
) {
    assert!(
        rank(variant) <= rank(canonical),
        "SECURITY HOLE ({label}): spelling variant resolves {variant:?}, more \
         permissive than the canonical form's {canonical:?} — allow-by-spelling-variant"
    );
}

struct Fixture {
    root: PathBuf,
    ws: PathBuf,
    private: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// temp root with `workspace/` (writable) holding `private/secret.txt`
/// (denied) and `ok.txt` (plain workspace file).
fn fixture(tag: &str) -> Fixture {
    let root = std::env::temp_dir().join(format!("nano-adv-policy-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let fixture = Fixture {
        ws: root.join("workspace"),
        private: root.join("workspace").join("private"),
        root,
    };
    std::fs::create_dir_all(&fixture.private).unwrap();
    std::fs::write(fixture.private.join("secret.txt"), "nano-adv-secret").unwrap();
    std::fs::write(fixture.ws.join("ok.txt"), "nano-adv-ok").unwrap();
    fixture
}

/// Root read + workspace write + explicit deny on `private`.
fn policy(f: &Fixture) -> FileSystemSandboxPolicy {
    FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(&f.ws).unwrap(),
            },
            FileSystemAccessMode::Write,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(&f.private).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ),
    ])
}

fn matcher(policy: &FileSystemSandboxPolicy, cwd: &Path) -> ReadDenyMatcher {
    ReadDenyMatcher::new(policy, cwd).expect("policy has deny-read restrictions")
}

/// Replaces a path segment's spelling (case-fold trick) without touching the
/// rest of the path, e.g. `workspace` -> `WORKSPACE`.
fn respell(path: &Path, from: &str, to: &str) -> PathBuf {
    let spelled = path.display().to_string();
    assert!(spelled.contains(from), "fixture path must contain {from}");
    PathBuf::from(spelled.replacen(from, to, 1))
}

fn drive_case_flipped(path: &Path) -> PathBuf {
    let spelled = path.display().to_string();
    let drive = &spelled[..1];
    let flipped = if drive.chars().next().unwrap().is_ascii_uppercase() {
        drive.to_ascii_lowercase()
    } else {
        drive.to_ascii_uppercase()
    };
    PathBuf::from(format!("{flipped}{}", &spelled[1..]))
}

// --- Case folding ------------------------------------------------------------

#[test]
fn case_variants_of_workspace_file_never_escalate_write() {
    let f = fixture("case-ws");
    let policy = policy(&f);
    let canonical = f.ws.join("ok.txt");
    assert_eq!(
        policy.resolve_access_with_cwd(&canonical, &f.ws),
        FileSystemAccessMode::Write,
        "control: canonical workspace file is writable"
    );

    let variants = [
        respell(&canonical, "workspace", "WORKSPACE"),
        respell(&canonical, "workspace", "WoRkSpAcE"),
        drive_case_flipped(&canonical),
    ];
    for variant in variants {
        let decision = policy.resolve_access_with_cwd(&variant, &f.ws);
        assert_not_more_permissive(
            FileSystemAccessMode::Write,
            decision,
            "case variant of writable root",
        );
    }
}

#[test]
fn case_variants_of_denied_dir_stay_read_denied() {
    let f = fixture("case-deny");
    let policy = policy(&f);
    let matcher = matcher(&policy, &f.ws);
    let canonical = f.private.join("secret.txt");
    assert!(
        matcher.is_read_denied(&canonical),
        "control: canonical denied"
    );

    let variants = [
        respell(&canonical, "private", "PRIVATE"),
        respell(&canonical, "private", "PrIvAtE"),
        respell(&canonical, "secret.txt", "SECRET.TXT"),
        drive_case_flipped(&canonical),
    ];
    for variant in variants {
        assert!(
            matcher.is_read_denied(&variant),
            "SECURITY HOLE (case variant): {} bypasses the read deny on {}",
            variant.display(),
            canonical.display()
        );
    }
}

// --- Verbatim / device / UNC prefixes --------------------------------------------

#[test]
fn verbatim_disk_prefix_spelling_matches_canonical_decision() {
    let f = fixture("verbatim");
    let policy = policy(&f);
    let matcher = matcher(&policy, &f.ws);

    for canonical in [f.private.join("secret.txt"), f.ws.join("ok.txt")] {
        let verbatim = PathBuf::from(format!(r"\\?\{}", canonical.display()));
        assert_eq!(
            policy.resolve_access_with_cwd(&verbatim, &f.ws),
            policy.resolve_access_with_cwd(&canonical, &f.ws),
            "SECURITY HOLE (verbatim prefix): {} diverges from {}",
            verbatim.display(),
            canonical.display()
        );
        assert_eq!(
            matcher.is_read_denied(&verbatim),
            matcher.is_read_denied(&canonical),
            "SECURITY HOLE (verbatim prefix read-deny): {}",
            verbatim.display()
        );
    }
}

#[test]
fn verbatim_unc_spelling_does_not_escape_to_a_local_grant() {
    // `\\?\UNC\host\share` is the verbatim spelling of the UNC path
    // `\\host\share`. The engine strips `\\?\` unconditionally, leaving a
    // relative-looking `UNC\host\share` tail that gets absolutized against the
    // PROCESS current directory — so the decision is computed for a phantom
    // local path while the OS would touch the network share.
    let cwd = std::env::current_dir().unwrap();
    let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry::new(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        },
        FileSystemAccessMode::Read,
    )]);

    let canonical = Path::new(r"\\nano-no-such-host\nanoshare\secret.txt");
    let canonical_decision = policy.resolve_access_with_cwd(canonical, &cwd);
    assert_eq!(
        canonical_decision,
        FileSystemAccessMode::Deny,
        "control: UNC path outside every entry must be denied"
    );

    let variant = Path::new(r"\\?\UNC\nano-no-such-host\nanoshare\secret.txt");
    let variant_decision = policy.resolve_access_with_cwd(variant, &cwd);
    assert_not_more_permissive(canonical_decision, variant_decision, r"\\?\UNC\ spelling");
}

#[test]
fn double_slash_spelling_does_not_inherit_local_grants() {
    // `//host/share` is a UNC path to Windows file APIs, but the engine's
    // Windows absolutize branch treats it as a root-relative local path and
    // re-roots it onto the cwd's drive: `//h/s` -> `D:\h\s`. The policy then
    // decides for a phantom local path while the OS would touch a share.
    let cwd = std::env::current_dir().unwrap();
    let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry::new(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        },
        FileSystemAccessMode::Read,
    )]);

    let canonical = Path::new(r"\\nano-no-such-host\nanoshare\secret.txt");
    let canonical_decision = policy.resolve_access_with_cwd(canonical, &cwd);
    assert_eq!(canonical_decision, FileSystemAccessMode::Deny);

    let variant = Path::new("//nano-no-such-host/nanoshare/secret.txt");
    let variant_decision = policy.resolve_access_with_cwd(variant, &cwd);
    assert_not_more_permissive(
        canonical_decision,
        variant_decision,
        "//host/share spelling",
    );
}

#[test]
fn device_path_spelling_is_not_more_permissive() {
    let f = fixture("device");
    let policy = policy(&f);

    // `\\.\D:\...` is the device-namespace spelling of the same local file.
    for canonical in [f.private.join("secret.txt"), f.ws.join("ok.txt")] {
        let device = PathBuf::from(format!(r"\\.\{}", canonical.display()));
        let canonical_decision = policy.resolve_access_with_cwd(&canonical, &f.ws);
        let variant_decision = policy.resolve_access_with_cwd(&device, &f.ws);
        assert_not_more_permissive(
            canonical_decision,
            variant_decision,
            r"\\.\ device spelling",
        );
        assert!(
            !policy.can_write_path_with_cwd(&device, &f.ws)
                || policy.can_write_path_with_cwd(&canonical, &f.ws),
            "SECURITY HOLE (device spelling): {}",
            device.display()
        );
    }
}

// --- Trailing dot / space, dot components, mixed separators ---------------------

#[test]
fn trailing_dot_and_space_spellings_stay_read_denied() {
    let f = fixture("trailing");
    let policy = policy(&f);
    let matcher = matcher(&policy, &f.ws);
    let canonical = f.private.join("secret.txt");
    assert!(
        matcher.is_read_denied(&canonical),
        "control: canonical denied"
    );

    let variants = [
        // Trailing dot / space on the file name (stripped by Windows file APIs).
        f.private.join("secret.txt."),
        f.private.join("secret.txt "),
        // Trailing dot / space on the denied directory component.
        f.ws.join("private.").join("secret.txt"),
        f.ws.join("private ").join("secret.txt"),
        // Trailing separator on the denied directory.
        f.private.join(".").join("secret.txt"),
    ];
    // A spelling variant only matters when the OS actually resolves it to the
    // protected bytes (e.g. Windows strips trailing dots; a trailing space on
    // a *directory* component empirically does NOT resolve — the policy's
    // decision about a path no filesystem call can open is moot). Canonicalize
    // equality is the OS-equivalence oracle.
    let canonical_target = std::fs::canonicalize(&canonical).unwrap();
    let mut asserted = 0;
    for variant in variants {
        let Ok(variant_target) = std::fs::canonicalize(&variant) else {
            eprintln!(
                "{}: OS does not resolve this spelling to the denied file; skipping",
                variant.display()
            );
            continue;
        };
        assert_eq!(
            variant_target,
            canonical_target,
            "test bug: {} is not the same file as the denied path",
            variant.display()
        );
        assert!(
            matcher.is_read_denied(&variant),
            "SECURITY HOLE (trailing dot/space): {} bypasses the read deny on {}",
            variant.display(),
            canonical.display()
        );
        asserted += 1;
    }
    assert!(
        asserted >= 3,
        "expected at least the dot variants to resolve"
    );
}

#[test]
fn dot_components_and_mixed_separators_match_canonical_decision() {
    let f = fixture("dots");
    let policy = policy(&f);
    let matcher = matcher(&policy, &f.ws);
    let canonical = f.private.join("secret.txt");
    let canonical_decision = policy.resolve_access_with_cwd(&canonical, &f.ws);
    assert_eq!(canonical_decision, FileSystemAccessMode::Deny);

    let sub = f.ws.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let forward_slashes = PathBuf::from(canonical.display().to_string().replace('\\', "/"));
    let variants = [
        sub.join("..").join("private").join("secret.txt"),
        f.ws.join(".").join("private").join("secret.txt"),
        forward_slashes,
    ];
    for variant in variants {
        assert_eq!(
            policy.resolve_access_with_cwd(&variant, &f.ws),
            canonical_decision,
            "SECURITY HOLE (dot/separator variant): {} diverges from canonical",
            variant.display()
        );
        assert!(
            matcher.is_read_denied(&variant),
            "SECURITY HOLE (dot/separator variant): {} bypasses the read deny",
            variant.display()
        );
    }
}

// --- NTFS alternate data streams ---------------------------------------------------

#[test]
fn ads_spellings_do_not_bypass_file_deny() {
    let root = std::env::temp_dir().join(format!("nano-adv-policy-{}-ads", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let denied_file = ws.join("secret.txt");
    std::fs::write(&denied_file, "nano-adv-secret").unwrap();

    // Deny the exact file; the rest of the workspace stays writable.
    let policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(&ws).unwrap(),
            },
            FileSystemAccessMode::Write,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(&denied_file).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ),
    ]);
    let matcher = matcher(&policy, &ws);
    assert!(
        matcher.is_read_denied(&denied_file),
        "control: canonical denied"
    );
    assert!(
        !policy.can_write_path_with_cwd(&denied_file, &ws),
        "control: canonical write denied"
    );

    // `file::$DATA` names the file's default stream — the SAME bytes the deny
    // protects (canonicalization normalizes it back to the plain file, so it
    // stays denied). `file:stream` is a separate stream on the same denied
    // file: it does not expose the denied bytes, but a deny on the file should
    // still cover all of its streams — otherwise hidden data can be attached
    // to (or read from) a denied file via ADS spelling.
    let default_stream = ws.join("secret.txt::$DATA");
    let named_stream = ws.join("secret.txt:nanoads");
    if std::fs::write(&named_stream, "nano-adv-ads").is_err() {
        eprintln!("skipping: ADS unavailable (non-NTFS volume)");
        let _ = std::fs::remove_dir_all(&root);
        return;
    }
    assert!(
        matcher.is_read_denied(&default_stream),
        "SECURITY HOLE (ADS default stream): {} reads the SAME bytes the deny \
         protects on {}",
        default_stream.display(),
        denied_file.display()
    );
    assert!(
        matcher.is_read_denied(&named_stream),
        "SECURITY HOLE (ADS named stream): {} escapes the deny on {} — hidden \
         streams on a denied file are outside the policy's view",
        named_stream.display(),
        denied_file.display()
    );
    assert!(
        !policy.can_write_path_with_cwd(&named_stream, &ws),
        "SECURITY HOLE (ADS named stream): {} escapes the write deny on {}",
        named_stream.display(),
        denied_file.display()
    );
    let _ = std::fs::remove_dir_all(&root);
}

// --- 8.3 short names -----------------------------------------------------------------

/// `dir /x` semantics via cmd's `%~sI` modifier. Returns None when the volume
/// has 8.3 name generation disabled (short name == long name). Uses `raw_arg`:
/// `Command::arg` would quote-wrap the command line and trip cmd's `/c`
/// quote-stripping, mangling the expansion.
fn short_name(path: &Path) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("cmd")
        .raw_arg("/c")
        .raw_arg(format!(r#"for %I in ("{}") do @echo %~sI"#, path.display()))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let spelled = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if spelled.is_empty() || spelled.eq_ignore_ascii_case(&path.display().to_string()) {
        return None;
    }
    Some(PathBuf::from(spelled))
}

#[test]
fn eight_three_short_name_spellings_stay_denied() {
    let f = fixture("8.3-names-long-segment");
    let policy = policy(&f);
    let matcher = matcher(&policy, &f.ws);
    let canonical = f.private.join("secret.txt");
    assert!(
        matcher.is_read_denied(&canonical),
        "control: canonical denied"
    );

    // Collect short-name spellings for the denied dir and file. Degrade
    // gracefully when 8.3 generation is off on this volume.
    let mut checked = 0;
    for (long, label) in [(&f.private, "denied dir"), (&canonical, "denied file")] {
        let Some(short) = short_name(long) else {
            eprintln!("skipping {label}: no 8.3 short name on this volume");
            continue;
        };
        let variant = if long.is_dir() {
            short.join("secret.txt")
        } else {
            short
        };
        assert!(
            matcher.is_read_denied(&variant),
            "SECURITY HOLE (8.3 short name): {} bypasses the read deny on {}",
            variant.display(),
            canonical.display()
        );
        checked += 1;
    }
    if checked == 0 {
        eprintln!("8.3 short names unavailable on this volume; nothing asserted");
    }
}

// --- Overlong paths (> MAX_PATH) ------------------------------------------------------

#[test]
fn overlong_path_spellings_match_canonical_decision() {
    let root =
        std::env::temp_dir().join(format!("nano-adv-policy-{}-overlong", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ws = root.join("workspace");
    let private = ws.join("private");
    std::fs::create_dir_all(&private).unwrap();

    // Build a > MAX_PATH (260 chars) tree under the denied dir via the
    // verbatim prefix, which bypasses the legacy limit for creation.
    let mut deep = private.clone();
    for _ in 0..12 {
        deep = deep.join("nano-deep-directory-segment");
    }
    let verbatim_deep = format!(r"\\?\{}", deep.display());
    std::fs::create_dir_all(&verbatim_deep).unwrap();
    let deep_file = deep.join("secret.txt");
    assert!(deep_file.display().to_string().len() > 260);
    std::fs::write(format!(r"\\?\{}", deep_file.display()), "nano-adv-secret").unwrap();

    let f = Fixture {
        root: root.clone(),
        ws,
        private,
    };
    let policy = policy(&f);
    let matcher = matcher(&policy, &f.ws);

    let canonical = deep_file.clone();
    let verbatim = PathBuf::from(format!(r"\\?\{}", deep_file.display()));
    let canonical_decision = policy.resolve_access_with_cwd(&canonical, &f.ws);
    assert_eq!(
        canonical_decision,
        FileSystemAccessMode::Deny,
        "control: overlong path under denied dir"
    );
    assert!(
        matcher.is_read_denied(&canonical),
        "SECURITY HOLE (overlong): {} bypasses the read deny",
        canonical.display()
    );
    assert_eq!(
        policy.resolve_access_with_cwd(&verbatim, &f.ws),
        canonical_decision,
        "SECURITY HOLE (overlong verbatim): decision diverges"
    );
    assert!(
        matcher.is_read_denied(&verbatim),
        "SECURITY HOLE (overlong verbatim): {} bypasses the read deny",
        verbatim.display()
    );
    drop(f);
}

// --- Relative vs absolute, unusual cwd -------------------------------------------------

#[test]
fn relative_spellings_from_unusual_cwd_match_absolute_decision() {
    let f = fixture("relative");
    let policy = policy(&f);
    let canonical = f.private.join("secret.txt");
    let canonical_decision = policy.resolve_access_with_cwd(&canonical, &f.ws);
    assert_eq!(canonical_decision, FileSystemAccessMode::Deny);

    // cwd inside the denied dir; bare filename.
    assert_eq!(
        policy.resolve_access_with_cwd(Path::new("secret.txt"), &f.private),
        canonical_decision,
        "SECURITY HOLE (relative-in-denied-cwd): bare name diverges"
    );
    // cwd in a sibling; `..` traversal back into the denied dir.
    let sub = f.ws.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    assert_eq!(
        policy.resolve_access_with_cwd(Path::new(r"..\private\secret.txt"), &sub),
        canonical_decision,
        "SECURITY HOLE (dotdot-relative): traversal spelling diverges"
    );
    // cwd spelled with wrong case: the absolute path derived through it
    // carries the wrong-case spelling, yet read-deny enforcement must still
    // catch it via canonicalization of the existing file.
    let upper_sub = respell(&sub, "workspace", "WORKSPACE");
    let matcher = matcher(&policy, &upper_sub);
    let case_shifted = AbsolutePathBuf::from_absolute_path(&upper_sub)
        .unwrap()
        .join(r"..\private\secret.txt");
    assert!(
        matcher.is_read_denied(case_shifted.as_path()),
        "SECURITY HOLE (case-shifted cwd): wrong-case spelling bypasses the read deny"
    );
    // A relative workspace file must not GAIN write by cwd-case confusion.
    assert!(
        !policy.can_write_path_with_cwd(
            Path::new("ok.txt"),
            &respell(&f.ws, "workspace", "WORKSPACE")
        ) || policy.can_write_path_with_cwd(Path::new("ok.txt"), &f.ws),
        "SECURITY HOLE (case-shifted cwd): relative write gained access"
    );
}

// --- NTFS hard links -----------------------------------------------------------
//
// A hard link is a second NAME for the same file object, not a reparse point:
// `canonicalize_write_target` sees a legitimate in-workspace path while an
// in-place write mutates the object under ALL of its names, including names
// outside every writable entry. The engine therefore denies writes to any
// existing regular file with more than one link. Evidence and analysis:
// `docs/audits/hardlink-race.md`.

#[test]
fn hard_link_alias_into_workspace_is_write_denied() {
    let f = fixture("hardlink-write");
    let policy = policy(&f);
    let outside = f.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let target = outside.join("target.txt");
    std::fs::write(&target, "nano-adv-original").unwrap();
    let link = f.ws.join("link.txt");
    // fs::hard_link needs no special privilege on Windows.
    std::fs::hard_link(&target, &link).unwrap();

    assert!(
        policy.can_write_path_with_cwd(&f.ws.join("ok.txt"), &f.ws),
        "control: plain in-workspace file is writable"
    );
    assert!(
        !policy.can_write_path_with_cwd(&link, &f.ws),
        "SECURITY HOLE (hard link): {} aliases the out-of-root object {}; \
         canonicalization cannot see the alias (no reparse point)",
        link.display(),
        target.display()
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "nano-adv-original",
        "outside target must be untouched"
    );
}

#[test]
fn hard_link_write_deny_releases_when_outside_name_is_removed() {
    // Pins that the deny tracks the LINK COUNT, not the name: once the
    // outside name is deleted the in-workspace name is the only name for the
    // object, and writing it no longer mutates anything outside the root.
    let f = fixture("hardlink-release");
    let policy = policy(&f);
    let outside = f.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let target = outside.join("target.txt");
    std::fs::write(&target, "nano-adv-original").unwrap();
    let link = f.ws.join("link.txt");
    std::fs::hard_link(&target, &link).unwrap();
    assert!(
        !policy.can_write_path_with_cwd(&link, &f.ws),
        "control: aliased file is write-denied"
    );

    std::fs::remove_file(&target).unwrap();
    assert!(
        policy.can_write_path_with_cwd(&link, &f.ws),
        "sole remaining name inside the writable root must be writable"
    );
}

#[test]
fn in_workspace_hard_link_pair_is_also_write_denied() {
    // Documented conservative false positive: BOTH names live inside the
    // writable root, so writing is containment-safe — but the engine cannot
    // enumerate a file's other names cheaply, so it denies any multi-linked
    // file. Callers must replace (unlink + create) such files instead of
    // editing them in place.
    let f = fixture("hardlink-inroot");
    let policy = policy(&f);
    let first = f.ws.join("first.txt");
    std::fs::write(&first, "nano-adv-pair").unwrap();
    let second = f.ws.join("second.txt");
    std::fs::hard_link(&first, &second).unwrap();

    for name in [&first, &second] {
        assert!(
            !policy.can_write_path_with_cwd(name, &f.ws),
            "multi-linked in-root file {} is denied in-place writes by design",
            name.display()
        );
    }
}

#[test]
fn hard_link_read_alias_transparency_is_pinned_as_platform_limitation() {
    // PLATFORM LIMITATION (docs/audits/hardlink-race.md): the lexical policy
    // layer cannot distinguish a hard link's names, so a read through an
    // in-workspace alias of a deny-read file is NOT denied here. Containment
    // is provided by the OS layer instead: the target file's DACL binds every
    // name of the object, and the sandbox identity has no read ACE on denied
    // targets (proven in nano-tools/tests/adversarial_fs.rs).
    let f = fixture("hardlink-read");
    let policy = policy(&f);
    let matcher = matcher(&policy, &f.ws);
    let target = f.private.join("secret.txt");
    let link = f.ws.join("link.txt");
    std::fs::hard_link(&target, &link).unwrap();

    assert!(matcher.is_read_denied(&target), "control: canonical denied");
    assert!(
        !matcher.is_read_denied(&link),
        "documented limitation: the read-deny matcher is alias-transparent \
         for hard links; the OS DACL layer is the containment for this"
    );
}

// --- Protected metadata under spelling variants -------------------------------------------

#[test]
fn git_metadata_case_and_trailing_dot_variants_stay_read_only() {
    let root = std::env::temp_dir().join(format!("nano-adv-policy-{}-meta", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ws = root.join("workspace");
    let git = ws.join(".git");
    std::fs::create_dir_all(&git).unwrap();
    std::fs::write(git.join("config"), "nano-adv-git").unwrap();

    let policy = workspace_write_policy(&[], true, true);
    let canonical = git.join("config");
    assert!(
        !policy.can_write_path_with_cwd(&canonical, &ws),
        "control: .git/config write is denied"
    );

    let variants = [
        ws.join(".GIT").join("config"),
        ws.join(".git.").join("config"),
        ws.join(".git ").join("config"), // trailing space strips to `.git` at the OS layer
        git.join("..").join(".git").join("config"),
    ];
    for variant in variants {
        assert!(
            !policy.can_write_path_with_cwd(&variant, &ws),
            "SECURITY HOLE (metadata spelling): {} bypasses .git write protection",
            variant.display()
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
