//! Library surface of the nanok3 binary crate: the ACP adapter is exposed so
//! integration tests can drive it in-process with scripted model/tool
//! doubles. The binaries (`nanok3`, `nanok3-acp-profile`) stay thin.

pub mod acp_mode;
pub mod flux_key;
