//! nano-model — provider-neutral model boundary.
//!
//! Nano-neutral request/event types with extensible metadata (never
//! Flux-specific fields in universal types). Wire policy (recorded from live
//! fixture evidence, shared/fixtures/flux/FINDINGS.md):
//! - PRIMARY: Flux Chat Completions at api.fluxrouter.ai/v1
//! - COMPAT: Flux Anthropic Messages (thinking/cache pass-through failed live
//!   — compat only, not the preferred route)
//! - DEFERRED: Responses (upstream marks it Phase 2)

pub mod flux_completions;
pub mod retry;
pub mod sse;
pub mod types;

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod live_smoke;

pub use types::{
    ContentBlock, ModelError, ModelEvent, ModelRequest, ModelResponse, Role, ToolCall,
    ToolDefinition, Usage,
};
