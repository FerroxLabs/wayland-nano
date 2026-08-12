//! nano-core — event types, errors, config, budgets, permission vocabulary.
//!
//! Constitution boundary: owns shared vocabulary only. No OS knowledge
//! (that is nano-platform), no network knowledge (that is nano-egress),
//! no provider knowledge (that is nano-model).

pub mod abs;
pub mod budget;
pub mod permissions;
pub mod policy_engine;
