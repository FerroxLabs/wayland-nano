//! P4 design §13 repomap battery + §14 leg 4 path-canonicalization
//! proofs, over a fixture workspace. Oracles are filesystem state, never
//! self-report.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::{
    FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry, FileSystemSandboxPolicy,
    FileSystemSpecialPath,
};
use nano_repomap::{
    IndexOptions, Language, ReadPolicy, RepoMap, SkipReason, SymbolKind, repomap_path_allowed_with,
};

fn write(ws: &Path, rel: &str, content: &str) -> PathBuf {
    let p = ws.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    p
}

/// No throttle/cadence by default in tests: every refresh is a full pass
/// unless a test overrides the cadence knobs.
fn test_options() -> IndexOptions {
    IndexOptions {
        refresh_throttle: Duration::ZERO,
        full_rehash_interval: Duration::ZERO,
        ..Default::default()
    }
}

fn base_entries(ws: &Path) -> Vec<FileSystemSandboxEntry> {
    vec![
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
    ]
}

fn base_policy(ws: &Path) -> FileSystemSandboxPolicy {
    FileSystemSandboxPolicy::restricted(base_entries(ws))
}

/// The standard fixture: rust + ts + markdown + .env + gitignored +
/// denied `secrets/` subtree.
fn fixture(deny_secrets: bool) -> (tempfile::TempDir, PathBuf, RepoMap) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write(
        &ws,
        "src/lib.rs",
        "pub fn greet_user(name: &str) -> String { format!(\"hi {name}\") }\npub struct Greeter;\n",
    );
    write(&ws, "src/main.rs", "fn main() { println!(\"x\"); }\n");
    write(
        &ws,
        "web/app.ts",
        "export class AppShell {}\nexport function boot() {}\n",
    );
    write(&ws, "README.md", "# Fixture\n\ntext\n");
    write(&ws, ".env", "TOKEN=placeholder\n");
    write(&ws, ".gitignore", "ignored.rs\n");
    write(&ws, "ignored.rs", "fn ignored_symbol() {}\n");
    write(&ws, "secrets/hidden.rs", "fn hidden_symbol() {}\n");

    let mut entries = base_entries(&ws);
    if deny_secrets {
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(ws.join("secrets")).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ));
    }
    let policy = FileSystemSandboxPolicy::restricted(entries);
    let map = RepoMap::build(&ws, test_options(), ReadPolicy::new(&policy, &ws)).unwrap();
    (tmp, ws, map)
}

#[test]
fn known_symbol_findable_by_name_token() {
    let (_tmp, _ws, mut map) = fixture(true);
    let result = map.query(Some("greet_user"), None, 20);
    assert_eq!(result.matches.len(), 1);
    let m = &result.matches[0];
    assert_eq!(m.name, "greet_user");
    assert_eq!(m.kind, SymbolKind::Function);
    assert_eq!(m.line, 1);
    assert!(m.path.ends_with("lib.rs"));
    assert!(!result.truncated);
    // Staleness honesty: a just-refreshed store labels its age.
    assert!(result.stats.last_refresh_age_ms.is_some());
    assert_eq!(result.stats.files, 5); // lib.rs, main.rs, app.ts, README.md, .gitignore
}

#[test]
fn multi_word_token_and() {
    let (_tmp, _ws, mut map) = fixture(true);
    // Both tokens must hit (name OR path): "greeter" (name) + "lib" (path).
    let result = map.query(Some("greeter lib"), None, 20);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.name == "Greeter" && m.path.ends_with("lib.rs"))
    );
    // "greeter" + "main" must NOT match the lib.rs symbol (AND, not OR).
    let result = map.query(Some("greeter main"), None, 20);
    assert!(result.matches.is_empty());
    // Qualified spelling tokenizes: "AppShell" via "web::appshell".
    let result = map.query(Some("web::appshell"), None, 20);
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.name == "AppShell" && m.kind == SymbolKind::Class)
    );
}

#[test]
fn unmatched_query_is_empty_honest_result_not_error() {
    let (_tmp, _ws, mut map) = fixture(true);
    let result = map.query(Some("no_such_symbol_anywhere"), None, 20);
    assert!(result.matches.is_empty());
    assert!(!result.truncated);
    assert!(result.stats.files > 0);
}

#[test]
fn path_glob_filters_results() {
    let (_tmp, _ws, mut map) = fixture(true);
    let glob = globset::GlobBuilder::new("src/**")
        .literal_separator(true)
        .build()
        .unwrap()
        .compile_matcher();
    let result = map.query(None, Some(&glob), 50);
    assert!(!result.matches.is_empty());
    assert!(result.matches.iter().all(|m| m.path.starts_with("src")
        || m.path.starts_with("src/")
        || m.path.starts_with("src\\")));
}

#[test]
fn truncation_is_explicit_never_silent() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let body = (0..30)
        .map(|i| format!("pub fn handler_{i}() {{}}\n"))
        .collect::<String>();
    write(&ws, "src/all.rs", &body);
    let policy = base_policy(&ws);
    let mut map = RepoMap::build(&ws, test_options(), ReadPolicy::new(&policy, &ws)).unwrap();
    let result = map.query(Some("handler"), None, 20);
    assert_eq!(result.matches.len(), 20);
    assert!(result.truncated, "the cut must be explicit");
    let full = map.query(Some("handler"), None, 50);
    assert_eq!(full.matches.len(), 30);
    assert!(!full.truncated);
}

#[test]
fn overline_file_recorded_with_empty_symbols() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write(&ws, "big.rs", &"fn f() {}\n".repeat(51_000));
    let policy = base_policy(&ws);
    let map = RepoMap::build(&ws, test_options(), ReadPolicy::new(&policy, &ws)).unwrap();
    let entry = map.entry(&ws.join("big.rs")).expect("recorded");
    assert_eq!(entry.skip, Some(SkipReason::Overline));
    assert_eq!(entry.lines, 50_001, "bounded streaming stops at 50_001");
    assert!(entry.symbols.is_empty());
    assert!(entry.content_hash.is_none());
}

#[test]
fn gitignored_path_absent() {
    let (_tmp, ws, map) = fixture(true);
    assert!(map.entry(&ws.join("ignored.rs")).is_none());
}

#[test]
fn denied_read_path_absent_counted_never_enumerated() {
    let (_tmp, ws, mut map) = fixture(true);
    // Absent from the index…
    assert!(map.entry(&ws.join("secrets/hidden.rs")).is_none());
    // …counted honestly (the dir itself counts once)…
    assert!(
        map.stats().skipped_denied >= 1,
        "skipped_denied = {}",
        map.stats().skipped_denied
    );
    // …and never enumerable through a query, even by exact symbol name.
    let result = map.query(Some("hidden_symbol"), None, 20);
    assert!(result.matches.is_empty());
}

#[test]
fn sensitive_env_absent() {
    let (_tmp, ws, map) = fixture(true);
    assert!(map.entry(&ws.join(".env")).is_none());
}

#[test]
fn edit_one_file_refresh_reextracts_exactly_that_file() {
    let (_tmp, ws, mut map) = fixture(true);
    std::fs::write(
        ws.join("src/lib.rs"),
        "pub fn greet_user(name: &str) -> String { format!(\"hi {name}\") }\npub struct Greeter;\npub fn added_after_refresh() {}\n",
    )
    .unwrap();
    map.refresh();
    assert_eq!(
        map.stats().refreshed_files,
        1,
        "exactly the edited file re-extracted"
    );
    let result = map.query(Some("added_after_refresh"), None, 20);
    assert_eq!(result.matches.len(), 1);
}

#[test]
fn deletion_is_picked_up_on_refresh() {
    let (_tmp, ws, mut map) = fixture(true);
    std::fs::remove_file(ws.join("src/main.rs")).unwrap();
    map.refresh();
    assert!(map.entry(&ws.join("src/main.rs")).is_none());
    let result = map.query(Some("main"), None, 20);
    assert!(result.matches.iter().all(|m| m.name != "main"));
}

#[test]
fn duplicate_content_indexed_independently_no_symbol_move() {
    let (_tmp, ws, mut map) = fixture(true);
    // Copy lib.rs while the original REMAINS: equal hash + old path still
    // present ⇒ duplicate, both indexed (§5.2 — hash equality cannot
    // distinguish copy from rename).
    std::fs::copy(ws.join("src/lib.rs"), ws.join("src/lib_copy.rs")).unwrap();
    map.refresh();
    let original = map.entry(&ws.join("src/lib.rs")).expect("original stays");
    let copy = map
        .entry(&ws.join("src/lib_copy.rs"))
        .expect("copy indexed");
    assert!(!original.symbols.is_empty() && !copy.symbols.is_empty());
    assert_eq!(original.symbols, copy.symbols);
}

#[test]
fn rename_proven_by_disappearance_rekeys_entry() {
    let (_tmp, ws, mut map) = fixture(true);
    let before = map
        .entry(&ws.join("src/lib.rs"))
        .expect("indexed")
        .symbols
        .clone();
    std::fs::rename(ws.join("src/lib.rs"), ws.join("src/renamed_lib.rs")).unwrap();
    map.refresh();
    assert!(map.entry(&ws.join("src/lib.rs")).is_none());
    let renamed = map.entry(&ws.join("src/renamed_lib.rs")).expect("re-keyed");
    assert_eq!(renamed.symbols, before, "same hash ⇒ symbols carried over");
}

#[test]
fn preserved_mtime_same_length_edit_caught_by_full_rehash() {
    // §13/§14 leg 4 [r2 codex-F9/F10; r3 claude-F7]: the mtime+len
    // shortcut is unsound — a same-length edit with the mtime restored
    // must still be caught within one full-rehash cadence (here forced
    // to every pass).
    let (_tmp, ws, mut map) = fixture(true);
    let file = ws.join("src/lib.rs");
    let original_mtime = std::fs::metadata(&file).unwrap().modified().unwrap();

    // Same-length edit: rename the function, pad with a space to hold len.
    let old = std::fs::read_to_string(&file).unwrap();
    let new = old.replacen("greet_user", "greet_peer", 1);
    assert_eq!(old.len(), new.len(), "fixture must be a same-length edit");
    std::fs::write(&file, &new).unwrap();
    // Restore the ORIGINAL mtime (std::fs::File::set_modified — the
    // portable spelling of the SetFileTime leg).
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(original_mtime)
        .unwrap();

    map.refresh();
    let result = map.query(Some("greet_peer"), None, 20);
    assert_eq!(
        result.matches.len(),
        1,
        "the hash pass must catch a preserved-mtime same-length edit"
    );
    let result = map.query(Some("greet_user"), None, 20);
    assert!(result.matches.is_empty());
}

#[test]
fn without_full_rehash_preserved_mtime_edit_is_stale_but_labeled() {
    // The negative pin for r2 codex-F9: with the full-rehash cadence
    // far away, a preserved-mtime same-length edit is NOT re-extracted —
    // this is exactly why the ≤ 30s hash cadence exists. The stale
    // window is bounded and the design states it; pin it so a future
    // regression to "never hashed" is loud.
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let file = write(&ws, "src/lib.rs", "pub fn alpha_one() {}\n");
    let policy = base_policy(&ws);
    let options = IndexOptions {
        refresh_throttle: Duration::ZERO,
        full_rehash_interval: Duration::from_secs(3600), // cadence far away
        ..Default::default()
    };
    let mut map = RepoMap::build(&ws, options, ReadPolicy::new(&policy, &ws)).unwrap();

    let original_mtime = std::fs::metadata(&file).unwrap().modified().unwrap();
    std::fs::write(&file, "pub fn alpha_two() {}\n").unwrap(); // same length
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(original_mtime)
        .unwrap();

    map.refresh();
    assert_eq!(map.stats().refreshed_files, 0);
    assert_eq!(
        map.query(Some("alpha_one"), None, 20).matches.len(),
        1,
        "stale within the cadence window — bounded by full_rehash_interval"
    );
}

#[test]
fn refresh_throttle_keeps_query_latency_flat() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write(&ws, "src/lib.rs", "pub fn throttled() {}\n");
    let policy = base_policy(&ws);
    let options = IndexOptions {
        refresh_throttle: Duration::from_secs(3600),
        ..Default::default()
    };
    let mut map = RepoMap::new(&ws, options, ReadPolicy::new(&policy, &ws)).unwrap();
    assert!(
        map.stats().last_refresh_age_ms.is_none(),
        "never-refreshed says so"
    );
    assert!(map.maybe_refresh(), "first query-triggered pass builds");
    assert!(
        !map.maybe_refresh(),
        "throttled: no second pass inside the window"
    );
}

#[test]
fn language_other_records_first_meaningful_line() {
    let (_tmp, ws, map) = fixture(true);
    let readme = map.entry(&ws.join("README.md")).expect("indexed");
    assert_eq!(readme.language, Language::Other);
    assert_eq!(readme.first_meaningful_line.as_deref(), Some("# Fixture"));
    assert!(readme.symbols.is_empty());
}

#[test]
fn case_and_separator_spellings_collapse_to_one_key() {
    // §14 leg 4 (cross-platform half): case and separator variants of
    // the same file are ONE canonical entry — on Windows (case-insensitive
    // NTFS). On unix the filesystem is case-sensitive, so a case variant
    // is a DIFFERENT (here: nonexistent) file and must NOT alias.
    let (_tmp, ws, map) = fixture(true);
    let canonical = map.entry(&ws.join("src/lib.rs")).expect("indexed");
    let spelled = ws.join("src").join("LIB.RS");
    #[cfg(target_os = "windows")]
    {
        let via_case = map.entry(&spelled).expect("case variant resolves");
        assert!(std::ptr::eq(canonical, via_case));
    }
    #[cfg(not(target_os = "windows"))]
    {
        assert!(
            map.entry(&spelled).is_none(),
            "unix: case variant must not alias a case-distinct file"
        );
    }
    // Forward-slash spelling of the same path.
    let slash = PathBuf::from(ws.join("src/lib.rs").to_string_lossy().replace('\\', "/"));
    let via_slash = map.entry(&slash).expect("slash variant resolves");
    assert!(std::ptr::eq(canonical, via_slash));
}

#[test]
fn repomap_path_allowed_with_matches_session_policy() {
    // The query-side spot check seam (§5.3) agrees with the walker.
    let (_tmp, ws, _map) = fixture(true);
    let policy_entries = {
        let mut entries = base_entries(&ws);
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(ws.join("secrets")).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ));
        entries
    };
    let policy = FileSystemSandboxPolicy::restricted(policy_entries);
    assert!(!repomap_path_allowed_with(
        &ws.join("secrets/hidden.rs"),
        &policy,
        &ws,
        false
    ));
    assert!(!repomap_path_allowed_with(
        &ws.join(".env"),
        &policy,
        &ws,
        false
    ));
    assert!(repomap_path_allowed_with(
        &ws.join("src/lib.rs"),
        &policy,
        &ws,
        false
    ));
}
