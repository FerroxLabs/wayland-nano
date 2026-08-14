//! Focused-window computer use. Synthesized input is not OS-contained;
//! callers must place every operation behind Nano's approval gate and journal.

pub mod backend;
pub mod backends;
pub mod coords;
pub mod error;
pub mod journal;
pub mod liveness;
#[doc(hidden)] // test support for the integrator's wiring tests — not a production backend
pub mod mock;
pub mod op;
pub mod policy;
pub mod posture;
pub mod redact;

pub use backend::{ComputerUseBackend, KeyMods, MouseButton, Platform, Region, ScreenshotFormat};
pub use error::{CuaError, CuaErrorKind, CuaResult};
pub use op::{CuaOp, CuaOpResult, NANO_CUA_OP_LOCKED_VARIANT_COUNT};
pub use policy::{CuaPolicy, CuaPolicyOutcome};

/// Env-mutating tests across modules serialize through this one lock —
/// the unit of contention is the test binary, not the module.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
