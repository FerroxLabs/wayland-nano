//! Fail-closed gate execution and independently preflightable red-green receipts.
//!
//! This crate is intentionally bottom-of-graph. It contains the WP-1 verification
//! primitives without depending on other Wayland Nano crates or exposing the WP-2
//! climb engine and WP-3 CLI surfaces.

pub mod error;
pub mod gate;
pub mod receipt;
pub mod registry;

pub use error::VerifyError;
