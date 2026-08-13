//! nano-repomap — aider-style LEXICAL symbol index (P4 design §5).
//!
//! A near-verbatim port of `wcore-repomap`'s extraction layer
//! (regex-per-language, Rust + TS/JS) over an in-memory per-session
//! store. The crate never escapes read policy (§5.3: the walker never
//! follows symlinks/junctions, `repomap_path_allowed` composes
//! denied-read + sensitive-path checks at every entry before any read),
//! performs NO filesystem writes (§5.4 — refresh is not workspace
//! mutation, discharged structurally in v1), and labels its own
//! staleness in every query result (§5.2).
//!
//! Deliberately NOT here (V2's minimal-deps in-memory v1): the `ignore`
//! crate (walker is the `nano-tools/src/search.rs` discipline + a
//! globset `.gitignore` approximation, deviation 8), rusqlite
//! persistence (§16 v2 seam), BM25/FTS ranking (lexical token-AND only),
//! auto-injection of the map into context (tool-only, §5.5).

pub mod extractor;
pub mod policy;
pub mod query;
pub mod store;
pub mod types;
pub mod walk;

pub use policy::{ReadPolicy, repomap_path_allowed, repomap_path_allowed_with};
pub use query::{QueryResult, RepoMapMatch};
pub use store::{FileEntry, IndexStats, RepoMap, SkipReason};
pub use types::{IndexOptions, Language, RepoMapError, Symbol, SymbolKind};
