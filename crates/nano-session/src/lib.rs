//! nano-session — append-only Op journal, replay, migrations, compaction.
//!
//! Kimi wire.jsonl model in Rust: serde-enum Ops, reducer-fold replay,
//! versioned migrations, unknown-record tolerance, crash recovery of
//! stranded phases. No remote compaction.
