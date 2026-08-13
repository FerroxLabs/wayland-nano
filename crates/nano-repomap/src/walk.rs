//! Workspace walker (P4 design §5.1): the `nano-tools/src/search.rs`
//! discipline extracted into this crate.
//!
//! Invariants (identical to `search.rs:64-103`):
//! - manual recursion that NEVER follows symlinks/junctions
//!   (`file_type().is_symlink() → skip`) — a junction pointing outside
//!   the workspace is never traversed, so no out-of-workspace path can
//!   enter the index (the structural half of §5.3);
//! - a dunce-canonical cycle guard per directory;
//! - the §5.3 read policy (`repomap_path_allowed`) checked at EVERY
//!   directory entry and BEFORE any read; denied paths are counted
//!   (`skipped_denied`), never enumerated;
//! - deterministic output (sorted).
//!
//! Gitignore respect is a globset-based approximation (deviation 8 —
//! the `ignore` crate is deliberately not a dependency): root + nested
//! `.gitignore` files, `!` negation, `/`-anchored vs basename patterns,
//! dir-only trailing `/`; last matching rule wins. Corner cases of full
//! gitignore semantics (escapes, `**` mid-pattern oddities) are not
//! guaranteed. `.git` directories are ALWAYS skipped, gitignore or not.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use globset::{GlobBuilder, GlobMatcher};

use crate::policy::{ReadPolicy, repomap_path_allowed};

/// Result of one walk pass.
#[derive(Debug, Default)]
pub struct WalkOutcome {
    /// Indexable files (policy-allowed, not ignored), sorted.
    pub files: Vec<PathBuf>,
    /// Paths skipped because the read policy denied them (dirs count
    /// once — their subtree is never descended into). Honest and
    /// bounded; the paths themselves are never reported (§5.3).
    pub skipped_denied: u64,
}

/// Walk `root` (already canonicalized by the caller) and collect every
/// indexable regular file.
pub fn walk(root: &Path, policy: &ReadPolicy, respect_gitignore: bool) -> WalkOutcome {
    let mut out = WalkOutcome::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut layers: Vec<GitignoreLayer> = Vec::new();
    walk_dir(
        root,
        policy,
        respect_gitignore,
        &mut seen,
        &mut layers,
        &mut out,
    );
    out.files.sort();
    out
}

fn walk_dir(
    dir: &Path,
    policy: &ReadPolicy,
    respect_gitignore: bool,
    seen: &mut HashSet<PathBuf>,
    layers: &mut Vec<GitignoreLayer>,
    out: &mut WalkOutcome,
) {
    let canonical = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !seen.insert(canonical) {
        return; // junction/symlink cycle guard (search.rs:70-73 discipline)
    }

    let pushed = respect_gitignore
        && repomap_path_allowed(&dir.join(".gitignore"), policy)
        && GitignoreLayer::load(dir).is_some_and(|layer| {
            layers.push(layer);
            true
        });

    let Ok(entries) = std::fs::read_dir(dir) else {
        if pushed {
            layers.pop();
        }
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_symlink() {
            continue; // never follow links (search.rs:85-87 discipline)
        }
        if ft.is_dir() {
            if entry.file_name() == ".git" {
                continue; // never index VCS internals
            }
            if !repomap_path_allowed(&path, policy) {
                out.skipped_denied += 1;
                continue;
            }
            if respect_gitignore && is_ignored(layers, &path, true) {
                continue;
            }
            walk_dir(&path, policy, respect_gitignore, seen, layers, out);
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        if !repomap_path_allowed(&path, policy) {
            out.skipped_denied += 1;
            continue;
        }
        if respect_gitignore && is_ignored(layers, &path, false) {
            continue;
        }
        out.files.push(path);
    }
    if pushed {
        layers.pop();
    }
}

/// One `.gitignore` file's rules, scoped to the directory containing it.
struct GitignoreLayer {
    base: PathBuf,
    rules: Vec<GitignoreRule>,
}

struct GitignoreRule {
    negated: bool,
    dir_only: bool,
    basename_only: bool,
    matcher: GlobMatcher,
}

impl GitignoreLayer {
    /// Parse `dir/.gitignore`. Returns `None` when absent/unreadable or
    /// when it yields no usable rules. Malformed lines are skipped (git's
    /// own stance); this is the documented approximation, never a
    /// fail-open on the READ POLICY (which is enforced separately).
    fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join(".gitignore")).ok()?;
        let mut rules = Vec::new();
        for raw in text.lines() {
            let line = raw.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negated, line) = match line.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            let (dir_only, line) = match line.strip_suffix('/') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            if line.is_empty() {
                continue;
            }
            // A `/` anywhere (leading included) anchors the pattern to
            // this layer's directory; otherwise it matches a basename at
            // any depth. Compute BEFORE stripping the leading `/`.
            let basename_only = !line.contains('/');
            let line = line.strip_prefix('/').unwrap_or(line);
            let Ok(glob) = GlobBuilder::new(line)
                .literal_separator(true)
                .allow_unclosed_class(true)
                .build()
            else {
                continue; // malformed pattern: skip the line, not the walk
            };
            rules.push(GitignoreRule {
                negated,
                dir_only,
                basename_only,
                matcher: glob.compile_matcher(),
            });
        }
        if rules.is_empty() {
            return None;
        }
        Some(Self {
            base: dir.to_path_buf(),
            rules,
        })
    }
}

/// Last matching rule across all active layers wins (`!` re-includes).
/// Deeper layers are later in the vec, so they override shallower ones.
fn is_ignored(layers: &[GitignoreLayer], path: &Path, is_dir: bool) -> bool {
    let mut ignored = false;
    for layer in layers {
        let Ok(rel) = path.strip_prefix(&layer.base) else {
            continue;
        };
        let rel_slash = slash_join(rel);
        for rule in &layer.rules {
            if rule.dir_only && !is_dir {
                continue;
            }
            let matched = if rule.basename_only {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| rule.matcher.is_match(n))
            } else {
                rule.matcher.is_match(&rel_slash)
            };
            if matched {
                ignored = !rule.negated;
            }
        }
    }
    ignored
}

/// `/`-joined relative path for glob matching (globset patterns are
/// separator-sensitive; normalize to the gitignore spelling).
fn slash_join(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_core::permissions::FileSystemSandboxPolicy;

    fn open_policy() -> ReadPolicy {
        ReadPolicy::new(&FileSystemSandboxPolicy::unrestricted(), Path::new("/"))
    }

    fn write(ws: &Path, rel: &str, content: &str) {
        let p = ws.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn rel_paths(out: &WalkOutcome, root: &Path) -> Vec<String> {
        out.files
            .iter()
            .map(|p| slash_join(p.strip_prefix(root).unwrap()))
            .collect()
    }

    #[test]
    fn collects_files_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        write(ws, "src/b.rs", "fn b() {}\n");
        write(ws, "src/a.rs", "fn a() {}\n");
        write(ws, "README.md", "# x\n");
        let out = walk(ws, &open_policy(), true);
        assert_eq!(
            rel_paths(&out, ws),
            vec!["README.md", "src/a.rs", "src/b.rs"]
        );
        assert_eq!(out.skipped_denied, 0);
    }

    #[test]
    fn gitignore_basename_and_dir_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        write(ws, ".gitignore", "target/\n*.log\n");
        write(ws, "src/main.rs", "fn main() {}\n");
        write(ws, "target/out.rs", "fn built() {}\n");
        write(ws, "debug.log", "noise\n");
        let out = walk(ws, &open_policy(), true);
        assert_eq!(rel_paths(&out, ws), vec![".gitignore", "src/main.rs"]);
    }

    #[test]
    fn gitignore_negation_reincludes() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        write(ws, ".gitignore", "*.log\n!keep.log\n");
        write(ws, "drop.log", "x\n");
        write(ws, "keep.log", "x\n");
        let out = walk(ws, &open_policy(), true);
        assert_eq!(rel_paths(&out, ws), vec![".gitignore", "keep.log"]);
    }

    #[test]
    fn gitignore_anchored_and_nested_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        write(ws, ".gitignore", "/rooted.txt\n");
        write(ws, "rooted.txt", "x\n");
        write(ws, "sub/rooted.txt", "x\n"); // not anchored — kept
        write(ws, "sub/.gitignore", "inner.rs\n");
        write(ws, "sub/inner.rs", "fn i() {}\n");
        write(ws, "inner.rs", "fn top() {}\n"); // nested layer must not leak up
        let out = walk(ws, &open_policy(), true);
        assert_eq!(
            rel_paths(&out, ws),
            vec![".gitignore", "inner.rs", "sub/.gitignore", "sub/rooted.txt"]
        );
    }

    #[test]
    fn gitignore_can_be_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        write(ws, ".gitignore", "ignored.rs\n");
        write(ws, "ignored.rs", "fn i() {}\n");
        let out = walk(ws, &open_policy(), false);
        assert_eq!(rel_paths(&out, ws), vec![".gitignore", "ignored.rs"]);
    }

    #[test]
    fn dot_git_never_indexed() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        write(ws, ".git/config", "[core]\n");
        write(ws, "src/main.rs", "fn main() {}\n");
        let out = walk(ws, &open_policy(), true);
        assert_eq!(rel_paths(&out, ws), vec!["src/main.rs"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_never_traversed() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.rs"), "fn secret() {}\n").unwrap();
        std::os::unix::fs::symlink(&outside, ws.join("linked")).unwrap();
        std::fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();
        let out = walk(&ws, &open_policy(), true);
        assert_eq!(rel_paths(&out, &ws), vec!["src/main.rs"]);
    }

    #[cfg(windows)]
    #[test]
    fn junction_pointing_outside_never_traversed() {
        // §14 leg 4: a junction is judged on its resolved target — an
        // out-of-workspace target is never traversed, so no
        // out-of-workspace path can enter the index.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.rs"), "fn secret() {}\n").unwrap();
        // Directory junctions need no elevation (unlike symlinks).
        let status = std::process::Command::new("cmd")
            .args([
                "/c",
                "mklink",
                "/J",
                &ws.join("linked").to_string_lossy(),
                &outside.to_string_lossy(),
            ])
            .status()
            .expect("spawn mklink");
        assert!(status.success(), "mklink /J failed: {status}");
        std::fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();
        let out = walk(&ws, &open_policy(), true);
        let rels = rel_paths(&out, &ws);
        assert_eq!(rels, vec!["src/main.rs"]);
        // fs oracle: nothing from the junction target leaked in.
        assert!(
            out.files
                .iter()
                .all(|p| !p.starts_with(&outside) && !slash_join(p).contains("secret"))
        );
    }
}
