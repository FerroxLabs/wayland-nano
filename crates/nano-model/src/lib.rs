//! nano-model — provider-neutral model boundary.
//!
//! Nano-neutral request/event types with extensible metadata (never
//! Flux-specific fields in universal types). Wire policy (recorded from live
//! fixture evidence, shared/fixtures/flux/FINDINGS.md):
//! - PRIMARY: Flux Chat Completions at api.fluxrouter.ai/v1
//! - COMPAT: Flux Anthropic Messages (thinking/cache pass-through failed live
//!   — FINDINGS batch-2 WIRE-2 — compat only, not the preferred route)
//! - IMPLEMENTED: Flux Responses (POST /v1/responses); Completions stays the
//!   single production wire per the same WIRE-2 verdict.

pub mod anthropic_messages;
pub mod auth;
pub mod catalog_schema;
pub mod flux_common;
pub mod flux_completions;
pub mod flux_grounding;
pub mod flux_models;
pub mod flux_responses;
pub mod metering;
pub mod params;
pub mod provider_catalog;
pub mod rate_limits;
pub mod retry;
pub mod sse;
pub mod structured;
pub mod types;

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod live_smoke;

pub use types::{
    ContentBlock, ModelError, ModelEvent, ModelRequest, ModelResponse, Role, ToolCall,
    ToolDefinition, Usage,
};
