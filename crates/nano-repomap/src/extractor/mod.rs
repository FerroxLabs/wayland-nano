//! Per-language symbol extractors and the shared dispatcher.
//!
//! Provenance: near-verbatim port of `wcore-repomap/src/extractor/mod.rs`
//! @ wayland-core-0.12.26 (module path only; donor tests retained).

pub mod rust;
pub mod typescript;

use crate::types::{Language, Symbol};

/// Dispatch to the right extractor for `language`. Returns `(symbols, imports)`.
/// For `Language::Other`, returns `(empty, empty)` — the indexer is expected
/// to record `first_meaningful_line` separately.
pub fn extract(language: Language, source: &str) -> (Vec<Symbol>, Vec<String>) {
    match language {
        Language::Rust => {
            let r = rust::extract_rust(source);
            (r.symbols, r.imports)
        }
        Language::TypeScript | Language::JavaScript => {
            let r = typescript::extract_typescript(source);
            (r.symbols, r.imports)
        }
        Language::Other => (Vec::new(), Vec::new()),
    }
}

/// Strip C-style line comments (`//…`) and block comments (`/* … */`)
/// from `source`, preserving the original line count and approximate
/// column positions so line-number reporting stays accurate.
///
/// String-literal awareness is intentionally NOT implemented — the
/// design contract specifies a "light" extractor, and false positives
/// inside string literals (e.g. a Rust file with the literal `"fn foo"`
/// in a doc test) are acceptable in exchange for the simplicity. The
/// fixture-index integration test asserts the behaviour on real code
/// shapes.
pub(crate) fn strip_comments_rust_style(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_block = false;
    while let Some(c) = chars.next() {
        if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                out.push_str("  "); // preserve column width loosely
                in_block = false;
            } else if c == '\n' {
                out.push('\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            // line comment — consume to next newline
            for nc in chars.by_ref() {
                if nc == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block = true;
            out.push_str("  ");
            continue;
        }
        out.push(c);
    }
    out
}

/// The first non-blank, non-comment line of `source`, truncated to 200
/// bytes on a char boundary. Recorded for `Language::Other` files only
/// (design §5.1).
///
/// Provenance: ported from `wcore-repomap/src/lib.rs::first_meaningful` —
/// Markdown-friendly (`#` headings are NOT skipped); only C-style
/// (`//`, `/*`, `*` continuation) and SQL-style (`--`) comment lines are.
pub(crate) fn first_meaningful(source: &str) -> Option<String> {
    for raw in source.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with("--")
        {
            continue;
        }
        let mut end = trimmed.len().min(200);
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        return Some(trimmed[..end].to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_comments_preserves_line_count() {
        let src = "fn a() {} // comment\n/* block\n   spans */ fn b() {}\n";
        let stripped = strip_comments_rust_style(src);
        assert_eq!(stripped.lines().count(), src.lines().count());
    }

    #[test]
    fn first_meaningful_skips_blanks_and_comments() {
        assert_eq!(
            first_meaningful("\n\n// note\n# Project\n"),
            Some("# Project".into())
        );
        assert_eq!(
            first_meaningful("/* block */\n* cont\n-- sql\nbody\n"),
            Some("body".into())
        );
        assert_eq!(first_meaningful("   \n"), None);
    }

    #[test]
    fn first_meaningful_truncates_on_char_boundary() {
        let long = format!("{}{}", "x".repeat(199), "é".repeat(10));
        let line = first_meaningful(&long).expect("a line");
        // 200 bytes would split the two-byte é; the cap backs off to 199.
        assert_eq!(line.len(), 199);
    }
}
