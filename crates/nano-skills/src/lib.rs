//! nano-skills — SKILL.md loading, parsing, scoped activation.
//!
//! Desktop owns discovery/catalog/trust. Nano owns load/parse/scoped
//! execution context. Vendored Codex parser base.

pub mod loader;
pub mod parser;

#[cfg(test)]
#[path = "parser_tests.rs"]
mod parser_tests;
