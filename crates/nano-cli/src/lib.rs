//! Library surface of the wayland-nano binary crate: the ACP adapter is exposed so
//! integration tests can drive it in-process with scripted model/tool
//! doubles. The binaries (`wayland-nano`, `wayland-nano-acp-profile`) stay thin.

pub mod acp_mode;
pub mod auto_routing;
pub mod cron_fire;
pub mod exec_mode;
pub mod exec_run;
pub mod fetch_specs;
pub mod flux_key;
pub mod mcp_specs;
pub mod model_params;
pub mod provider_key;
pub mod provider_router;
pub mod review_diff;
pub mod search_specs;
pub mod session_browser;
pub mod session_cmds;
pub mod session_tools;

#[cfg(test)]
#[path = "exec_tests.rs"]
mod exec_tests;
