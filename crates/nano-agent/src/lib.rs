//! nano-agent — turn loop, tool router, approvals, bounded subagents.
//!
//! The loop never sees OS details (nano-platform) or wire details
//! (nano-model). Loop protection: repeated-action breaker, no-progress
//! detection, failure streaks, budgets.

pub mod loop_protection;
pub mod turn;

#[cfg(test)]
#[path = "turn_tests.rs"]
mod turn_tests;
