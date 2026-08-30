//! Library surface of the wayland-nano binary crate: the ACP adapter is exposed so
//! integration tests can drive it in-process with scripted model/tool
//! doubles. The binaries (`wayland-nano`, `wayland-nano-acp-profile`) stay thin.

pub mod acp_mode;
pub mod activation;
pub mod auth_cmds;
pub mod auto_routing;
pub mod cron_fire;
pub mod exec_mode;
pub mod exec_run;
pub mod fetch_specs;
pub mod flux_key;
pub mod mcp_specs;
pub mod model_params;
pub mod plugin_cmds;
pub mod provider_key;
pub mod provider_router;
pub mod review_diff;
pub mod rules_cmds;
pub mod search_specs;
pub mod session_browser;
pub mod session_cmds;
pub mod session_tools;
pub mod shell_rules;
pub mod verify_cmd;

#[cfg(test)]
#[path = "exec_tests.rs"]
mod exec_tests;
#[cfg(test)]
#[path = "plugin_activation_tests.rs"]
mod plugin_activation_tests;
