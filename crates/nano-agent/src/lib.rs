//! nano-agent — turn loop, tool router, approvals, bounded subagents.
//!
//! The loop never sees OS details (nano-platform) or wire details
//! (nano-model). Loop protection: repeated-action breaker, no-progress
//! detection, failure streaks, budgets.

pub mod compact;
pub mod loop_protection;
pub mod mcp;
pub mod skills;
pub mod steer;
pub mod turn;
pub mod wiring;

#[cfg(test)]
#[path = "turn_tests.rs"]
mod turn_tests;

#[cfg(test)]
#[path = "c9_tests.rs"]
mod c9_tests;
