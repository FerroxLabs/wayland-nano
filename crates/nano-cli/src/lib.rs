//! Library surface of the wayland-nano binary crate: the ACP adapter is exposed so
//! integration tests can drive it in-process with scripted model/tool
//! doubles. The binaries (`wayland-nano`, `wayland-nano-acp-profile`) stay thin.

pub mod acp_mode;
pub mod fetch_specs;
pub mod flux_key;
pub mod mcp_specs;
pub mod session_tools;
