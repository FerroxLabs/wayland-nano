//! Public data types for `nano-repomap`.
//!
//! Provenance: adapted from `wcore-repomap/src/types.rs` @
//! wayland-core-0.12.26. Transformations: serde derives dropped (v1 is
//! in-memory only — P4 design §5.2, rusqlite persistence deferred);
//! `RepoMap`/`FileSummary` replaced by the keyed store in `store.rs`
//! (per-session `BTreeMap<PathKey, FileEntry>` with freshness metadata);
//! `IndexOptions` gains the freshness cadence and refresh-bound knobs
//! (design §5.2 r2 codex-F9 + F-28 MEDIUM-3).
//! `Symbol`/`SymbolKind`/`Language` are near-verbatim.

use std::path::Path;
use std::time::Duration;

/// One extracted symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// Symbol name as it appears in source (e.g. `Greeter`, `hello_world`).
    /// For `impl Trait for Type`, this is `"Trait for Type"`.
    pub name: String,
    /// 1-based line number where the declaration starts.
    pub line: usize,
}

/// Symbol kinds covered by the extractors (design §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    // Rust + TypeScript-shared
    /// Function declaration (`fn` in Rust, `function` in TS/JS).
    Function,
    /// Struct declaration (Rust only).
    Struct,
    /// Enum declaration (Rust only).
    Enum,
    // Rust-specific
    /// Trait declaration.
    Trait,
    /// `impl` block (inherent or `impl Trait for Type`).
    Impl,
    /// Module declaration.
    Module,
    /// `pub use` re-export.
    Use,
    // TypeScript-specific
    /// Class declaration.
    Class,
    /// Interface declaration.
    Interface,
    /// Type alias.
    TypeAlias,
    /// `export const/let/var` or `export { … }` re-export.
    Export,
}

impl SymbolKind {
    /// Stable snake_case label for rendering (was the serde wire name in
    /// the donor; v1 has no serde, so this is the single spelling).
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Module => "module",
            SymbolKind::Use => "use",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::TypeAlias => "type_alias",
            SymbolKind::Export => "export",
        }
    }
}

/// Language tag derived from file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// `.rs`
    Rust,
    /// `.ts`, `.tsx`
    TypeScript,
    /// `.js`, `.mjs`, `.cjs`, `.jsx`
    JavaScript,
    /// Anything else — first-line + size fallback only.
    Other,
}

impl Language {
    /// Map a path's extension to a `Language`. Case-insensitive on the
    /// extension. Returns `Language::Other` for unknown extensions.
    pub fn from_path(path: &Path) -> Self {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_ascii_lowercase(),
            None => return Language::Other,
        };
        match ext.as_str() {
            "rs" => Language::Rust,
            "ts" | "tsx" => Language::TypeScript,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            _ => Language::Other,
        }
    }
}

/// Options for indexing and freshness. Defaults are the design values
/// (§5.1 caps, §5.2 cadences).
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Maximum file size to scan (bytes). Metadata rejects known-large
    /// files before open; the streaming reader admits one extra byte to
    /// detect growth after stat. Larger files are recorded with empty
    /// symbols. Default 5 MB.
    pub max_file_bytes: u64,
    /// Maximum line count to scan. Enforced by a BOUNDED STREAMING read:
    /// lines are counted while reading and the read STOPS at line
    /// `max_lines + 1` (design §5.1 r2 codex-F9 — line count is not
    /// knowable from metadata). Over-line files are recorded with empty
    /// symbols. Default 50_000.
    pub max_lines: usize,
    /// Maximum policy-allowed files retained and processed by one
    /// refresh pass. Additional files are counted in
    /// `IndexStats::skipped_over_cap`. Default 10,000.
    pub max_files_per_refresh: usize,
    /// If true, respect `.gitignore` files (globset-based approximation —
    /// deviation 8; negation/nesting corner cases of full gitignore
    /// semantics are not guaranteed). `.git` itself is always skipped.
    /// Default true.
    pub respect_gitignore: bool,
    /// Minimum spacing between query-triggered refresh passes (design
    /// §5.2: one refresh pass per 2s, keeping query latency flat).
    /// Default 2s.
    pub refresh_throttle: Duration,
    /// Full tracked-file hash-pass cadence (design §5.2 r2 codex-F9:
    /// once per 30s window every tracked file is re-hashed, so a
    /// preserved-mtime/same-length edit is caught within one full-rehash
    /// cadence). Default 30s.
    pub full_rehash_interval: Duration,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 5 * 1024 * 1024,
            max_lines: 50_000,
            max_files_per_refresh: 10_000,
            respect_gitignore: true,
            refresh_throttle: Duration::from_secs(2),
            full_rehash_interval: Duration::from_secs(30),
        }
    }
}

/// Public error type for the crate. Only CONSTRUCTION failures are typed;
/// per-entry IO errors during indexing are recorded on the entry and
/// skipped (design §5.1: "never fatal").
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RepoMapError {
    /// The provided root did not exist or could not be canonicalized.
    #[error("repo root not accessible: {path}: {source}")]
    Root {
        /// Path that failed to canonicalize.
        path: std::path::PathBuf,
        /// Underlying IO error.
        #[source]
        source: std::io::Error,
    },
}
