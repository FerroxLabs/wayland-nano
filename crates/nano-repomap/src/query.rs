//! Lexical query over the store (P4 design §5.5): token-AND over symbol
//! names + path substrings, deterministic ranking, explicit truncation.
//! NO BM25/FTS in v1 (V2 "lexical query only" — the wcore measured
//! failure mode is the doctrine).

use std::path::PathBuf;

use globset::GlobMatcher;

use crate::store::{IndexStats, RepoMap};
use crate::types::SymbolKind;

/// One matched symbol. `path` is relative to the store root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMapMatch {
    pub path: PathBuf,
    pub line: usize,
    pub kind: SymbolKind,
    pub name: String,
}

/// Query result: matches (bounded), the honest freshness stats, and an
/// EXPLICIT truncation flag — never a silent cut (§5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    pub matches: Vec<RepoMapMatch>,
    pub stats: IndexStats,
    pub truncated: bool,
}

impl RepoMap {
    /// Lexical query. Triggers the throttled refresh first (§5.2 — a
    /// stale answer is labeled via `stats`, and the throttle keeps query
    /// latency flat).
    ///
    /// - `text`: free-text query, split into lowercase alphanumeric
    ///   tokens; a symbol matches when EVERY token is a substring of the
    ///   symbol name OR of the file's canonical path key (token-AND).
    ///   `None` matches every symbol (useful with `path_glob`).
    /// - `path_glob`: pre-compiled glob (the caller validates and types
    ///   the error) matched against the root-relative path, same
    ///   matcher discipline as `nano-tools` search.
    /// - `max_results`: hard bound; the wrapper clamps to [1, 50].
    pub fn query(
        &mut self,
        text: Option<&str>,
        path_glob: Option<&GlobMatcher>,
        max_results: usize,
    ) -> QueryResult {
        self.maybe_refresh();

        let tokens = tokenize(text.unwrap_or(""));
        let exact = text.map(str::trim).filter(|q| !q.is_empty());

        let mut scored: Vec<(u8, usize, usize, PathBuf, RepoMapMatch)> = Vec::new();
        for (key, entry) in self.entries() {
            let rel = entry
                .path
                .strip_prefix(self.root())
                .unwrap_or(&entry.path)
                .to_path_buf();
            if let Some(glob) = path_glob
                && !glob.is_match(&rel)
            {
                continue;
            }
            let depth = rel.components().count();
            for sym in &entry.symbols {
                let name_lower = sym.name.to_ascii_lowercase();
                if !tokens
                    .iter()
                    .all(|t| name_lower.contains(t.as_str()) || key.contains(t.as_str()))
                {
                    continue;
                }
                let m = RepoMapMatch {
                    path: rel.clone(),
                    line: sym.line,
                    kind: sym.kind,
                    name: sym.name.clone(),
                };
                // Ranking (§5.5): exact-name match first, then path
                // depth, then line; path+name give a total,
                // deterministic order.
                let exact_rank = u8::from(exact.is_none_or(|q| !sym.name.eq_ignore_ascii_case(q)));
                scored.push((exact_rank, depth, sym.line, rel.clone(), m));
            }
        }
        scored.sort_by(|a, b| {
            (&a.0, &a.1, &a.2, &a.3, &a.4.name).cmp(&(&b.0, &b.1, &b.2, &b.3, &b.4.name))
        });

        let truncated = scored.len() > max_results;
        scored.truncate(max_results);
        QueryResult {
            matches: scored.into_iter().map(|(.., m)| m).collect(),
            stats: self.stats(),
            truncated,
        }
    }
}

/// Lowercase alphanumeric/`_`/`$` tokens: `RepoMap::new` queries as
/// ["repomap", "new"], so a qualified spelling still token-ANDs against
/// name + path.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_splits_qualified_spellings() {
        assert_eq!(tokenize("RepoMap::new"), vec!["repomap", "new"]);
        assert_eq!(tokenize("greeter  build"), vec!["greeter", "build"]);
        assert_eq!(tokenize(""), Vec::<String>::new());
        assert_eq!(tokenize("$el.class"), vec!["$el", "class"]);
    }
}
