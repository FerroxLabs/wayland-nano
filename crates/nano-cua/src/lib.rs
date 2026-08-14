//! Focused-window computer use. Synthesized input is not OS-contained;
//! callers must place every operation behind Nano's approval gate and journal.

pub mod backend;
pub mod backends;
pub mod error;
pub mod liveness;
pub mod op;
pub mod policy;
pub mod redact;

pub use backend::{ComputerUseBackend, KeyMods, MouseButton, Platform, Region, ScreenshotFormat};
pub use error::{CuaError, CuaErrorKind, CuaResult};
pub use op::{CuaOp, CuaOpResult, NANO_CUA_OP_LOCKED_VARIANT_COUNT};
pub use policy::{CuaPolicy, CuaPolicyOutcome};
