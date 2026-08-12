//! nano-tools — filesystem read/write/edit, patch, search/glob, shell, git.
//!
//! One policy/event path for every tool. No raw process spawning here —
//! processes go through nano-platform SpawnSpec only.

pub mod fs;
pub mod search;
pub mod shell;
pub mod web;
pub mod web_search;
