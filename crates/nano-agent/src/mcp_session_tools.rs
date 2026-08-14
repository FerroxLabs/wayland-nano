//! P3 §3.2/§4.3: execution of the MCP session tools (`tool_search`,
//! `mcp_list_resources`, `mcp_read_resource`) — ONE implementation shared by
//! every host surface (ACP `SessionTools`, exec, protocol host). The
//! [`McpSessionToolExecutor`] wrapper routes exactly the
//! `MCP_SESSION_TOOL_NAMES` set and delegates everything else.
//!
//! Hydration is JOURNAL-FIRST (§3.3): the search computes the bounded batch,
//! the `Op::McpToolHydration` append lands durably through the session's
//! `JournalCoordinator`, and ONLY then does the registry's hydrated set
//! mutate. Append failure ⇒ typed `JournalUnavailable`, set unchanged.

use crate::loop_protection::ProgressSignals;
use crate::mcp::{McpRegistry, ResourceError};
use crate::turn::{ToolExecutor, ToolOutcome};
use nano_model::types::ToolCall;
use nano_session::op::{Op, OpEnvelope};
use nano_session::{JournalCoordinator, NanoErrorKind, validate_hydration_batch};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex};

fn outcome(ok: bool, output: String, kind: Option<NanoErrorKind>) -> ToolOutcome {
    ToolOutcome {
        ok,
        output,
        progress: ProgressSignals {
            new_information: ok,
            ..Default::default()
        },
        error_kind: kind,
    }
}

fn resource_failure(err: ResourceError) -> ToolOutcome {
    outcome(false, err.message, Some(err.kind))
}

/// `tool_search` (§3.2): DiscoveryLocal — a LOCAL index search plus the
/// journaled exposure mutation; no server contact. Double-guarded: a forced
/// invocation with nothing deferred is a typed refusal.
pub fn execute_tool_search(
    registry: &Arc<Mutex<McpRegistry>>,
    coordinator: &JournalCoordinator,
    op_id: String,
    query: &str,
    cancel: Option<&AtomicBool>,
) -> ToolOutcome {
    if query.trim().is_empty() {
        return outcome(
            false,
            "tool_search rejected: query must be a non-empty string".into(),
            Some(NanoErrorKind::InvalidParams),
        );
    }
    {
        let guard = registry.lock().unwrap_or_else(|p| p.into_inner());
        // §3.2 double-guard: unadvertised AND a typed refusal on forced
        // invocation when no deferred inventory exists (the P1 D3 pattern).
        if !guard.has_deferred_tools() {
            return outcome(
                false,
                "tool_search refused: no deferred MCP tools exist in this session".into(),
                Some(NanoErrorKind::UnknownTool),
            );
        }
    }
    let result = registry
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .tool_search(query, cancel);
    let found = match result {
        Ok(found) => found,
        Err(kind) => {
            return outcome(false, format!("tool_search failed: {kind:?}"), Some(kind));
        }
    };
    if !found.hydration.is_empty() {
        // Journal-first (§3.3): validate the batch, append ONE atomic op,
        // and only then mutate the hydrated set. Append failure ⇒ typed
        // JournalUnavailable, set unchanged.
        if let Err(rule) = validate_hydration_batch(&found.hydration) {
            return outcome(
                false,
                format!("tool_search refused: hydration batch violates journal bounds ({rule})"),
                Some(NanoErrorKind::InvalidParams),
            );
        }
        let envelope = OpEnvelope::new(
            op_id.clone(),
            "now",
            Op::McpToolHydration {
                hydration_id: op_id,
                entries: found.hydration.clone(),
            },
        );
        if let Err(err) = coordinator.append(&envelope) {
            return outcome(
                false,
                format!("tool_search: journal append failed ({err}); no tools were loaded"),
                Some(NanoErrorKind::JournalUnavailable),
            );
        }
        registry
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .apply_hydration(&found.hydration);
    }
    let mut lines: Vec<String> = found
        .hits
        .iter()
        .map(|hit| hit.namespaced.clone())
        .collect();
    if found.more > 0 {
        lines.push(format!("{} more matches — refine your query", found.more));
    }
    lines.extend(found.notices.iter().cloned());
    if found.hits.is_empty() {
        lines.push("no deferred tools matched".to_string());
    } else {
        lines.push(found.status.clone());
    }
    outcome(true, lines.join("\n"), None)
}

/// `mcp_list_resources` (§4.3): ServerQuery — a read-only wire call through
/// the dispatcher, capability-gated before any wire activity. Lock
/// discipline (§2.1 defect 5 / §5.2): the registry lock is held only to
/// gate + clone the client and (after the round trip) to refresh the
/// advertised-URI cache — never across the blocking call.
pub fn execute_list_resources(
    registry: &Arc<Mutex<McpRegistry>>,
    server: Option<&str>,
) -> ToolOutcome {
    let Some(server) = server.filter(|s| !s.trim().is_empty()) else {
        return outcome(
            false,
            "mcp_list_resources rejected: server must be a non-empty string".into(),
            Some(NanoErrorKind::InvalidParams),
        );
    };
    let client = {
        let guard = registry.lock().unwrap_or_else(|p| p.into_inner());
        match guard.resource_client_for(server) {
            Ok(client) => client,
            Err(err) => return resource_failure(err),
        }
    };
    // NO registry lock held across the round trip.
    let result = match client.list_resources() {
        Ok(result) => result,
        Err(err) => return resource_failure(crate::mcp::resource_error_of_mcp(err)),
    };
    let notices = registry
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .cache_resource_listing(server, &result);
    let mut lines: Vec<String> = result
        .resources
        .iter()
        .map(|resource| {
            // LOW-6/§3.6: server-authored URI/description are untrusted
            // remote content — sanitized (control chars stripped), capped,
            // and the block is labeled untrusted.
            let (uri, _) = crate::mcp::sanitize_description(Some(&resource.uri));
            let (description, _) =
                crate::mcp::sanitize_description(resource.description.as_deref());
            let mime = resource.mime_type.as_deref().unwrap_or("unknown");
            match description {
                Some(description) if !description.is_empty() => {
                    format!("{} ({}) — {}", uri.unwrap_or_default(), mime, description)
                }
                _ => format!("{} ({})", uri.unwrap_or_default(), mime),
            }
        })
        .collect();
    if result.truncated() {
        lines.push(
            "truncated: the server has more resources (pagination is not followed in v1)"
                .to_string(),
        );
    }
    lines.extend(notices);
    if result.resources.is_empty() {
        lines.push(format!("server '{server}' advertised no resources"));
    }
    outcome(
        true,
        format!(
            "[Untrusted MCP resource listing from server '{server}']\n{}",
            lines.join("\n")
        ),
        None,
    )
}

/// `mcp_read_resource` (§4.3): ServerDataRead — advertised-URI-gated (no
/// wire call for an unadvertised URI), text-only, bounded, labeled
/// untrusted in the render (§9.1).
pub fn execute_read_resource(
    registry: &Arc<Mutex<McpRegistry>>,
    server: Option<&str>,
    uri: Option<&str>,
) -> ToolOutcome {
    let (Some(server), Some(uri)) = (
        server.filter(|s| !s.trim().is_empty()),
        uri.filter(|s| !s.trim().is_empty()),
    ) else {
        return outcome(
            false,
            "mcp_read_resource rejected: server and uri must be non-empty strings".into(),
            Some(NanoErrorKind::InvalidParams),
        );
    };
    let client = {
        let guard = registry.lock().unwrap_or_else(|p| p.into_inner());
        let client = match guard.resource_client_for(server) {
            Ok(client) => client,
            Err(err) => return resource_failure(err),
        };
        // The advertised-URI gate runs BEFORE any wire call, under the same
        // short lock.
        if let Err(err) = guard.validate_advertised_uri(server, uri) {
            return resource_failure(err);
        }
        client
    };
    // NO registry lock held across the round trip.
    let result = client.read_resource(uri);
    match result {
        Ok(result) => {
            let mut text = String::new();
            for content in &result.contents {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&content.text);
            }
            outcome(
                true,
                format!("[Untrusted MCP resource content from server '{server}' ({uri})]\n{text}"),
                None,
            )
        }
        Err(err) => resource_failure(crate::mcp::resource_error_of_mcp(err)),
    }
}

/// The host-surface executor wrapper: services exactly the
/// `MCP_SESSION_TOOL_NAMES` set against the shared registry, journal-first;
/// everything else delegates. Hosts without an MCP registry answer the three
/// names with a typed unavailability (never a silent empty result — the
/// wcore #504 rule).
pub struct McpSessionToolExecutor<'a> {
    registry: Option<Arc<Mutex<McpRegistry>>>,
    coordinator: Arc<JournalCoordinator>,
    session_id: String,
    /// Monotonic per-lifetime counter for hydration op ids (nanos in the id
    /// keep resumes collision-free — the C2 ModeSet pattern).
    op_counter: AtomicU64,
    inner: &'a dyn ToolExecutor,
}

impl std::fmt::Debug for McpSessionToolExecutor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpSessionToolExecutor")
            .field("session_id", &self.session_id)
            .field("has_registry", &self.registry.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> McpSessionToolExecutor<'a> {
    pub fn new(
        registry: Option<Arc<Mutex<McpRegistry>>>,
        coordinator: Arc<JournalCoordinator>,
        session_id: String,
        inner: &'a dyn ToolExecutor,
    ) -> Self {
        Self {
            registry,
            coordinator,
            session_id,
            op_counter: AtomicU64::new(0),
            inner,
        }
    }

    fn next_op_id(&self) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = self
            .op_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("{}-hydrate-{nanos}-{n}", self.session_id)
    }

    fn unavailable(&self, name: &str) -> ToolOutcome {
        outcome(
            false,
            format!(
                "{name} unavailable: this host registered no MCP registry for the session wrapper"
            ),
            Some(NanoErrorKind::UnknownTool),
        )
    }
}

#[async_trait::async_trait]
impl ToolExecutor for McpSessionToolExecutor<'_> {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        let Some(registry) = &self.registry else {
            if crate::wiring::MCP_SESSION_TOOL_NAMES.contains(&call.name.as_str()) {
                return self.unavailable(&call.name);
            }
            return self.inner.execute(call).await;
        };
        match call.name.as_str() {
            "tool_search" => {
                let query = call.arguments.get("query").and_then(|v| v.as_str());
                execute_tool_search(
                    registry,
                    &self.coordinator,
                    self.next_op_id(),
                    query.unwrap_or(""),
                    None,
                )
            }
            "mcp_list_resources" => execute_list_resources(
                registry,
                call.arguments.get("server").and_then(|v| v.as_str()),
            ),
            "mcp_read_resource" => execute_read_resource(
                registry,
                call.arguments.get("server").and_then(|v| v.as_str()),
                call.arguments.get("uri").and_then(|v| v.as_str()),
            ),
            _ => self.inner.execute(call).await,
        }
    }

    /// The cancel flag reaches tool_search's scan (cancel-checked every 100
    /// items, §3.2); the resource arms are single bounded round-trips that
    /// complete at the dispatcher's own deadlines.
    async fn execute_cancellable(
        &self,
        call: &ToolCall,
        cancel: Option<&AtomicBool>,
    ) -> ToolOutcome {
        let Some(registry) = &self.registry else {
            if crate::wiring::MCP_SESSION_TOOL_NAMES.contains(&call.name.as_str()) {
                return self.unavailable(&call.name);
            }
            return self.inner.execute_cancellable(call, cancel).await;
        };
        match call.name.as_str() {
            "tool_search" => {
                let query = call.arguments.get("query").and_then(|v| v.as_str());
                execute_tool_search(
                    registry,
                    &self.coordinator,
                    self.next_op_id(),
                    query.unwrap_or(""),
                    cancel,
                )
            }
            "mcp_list_resources" | "mcp_read_resource" => self.execute(call).await,
            _ => self.inner.execute_cancellable(call, cancel).await,
        }
    }

    /// F-P3-5: delegate the mid-turn hydration refresh to the inner
    /// MCP-merged executor (this wrapper owns no tool definitions itself).
    fn current_mcp_tool_definitions(&self) -> Option<Vec<nano_model::types::ToolDefinition>> {
        self.inner.current_mcp_tool_definitions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::ApprovalDecision;

    #[derive(Debug)]
    struct NoopInner;
    #[async_trait::async_trait]
    impl ToolExecutor for NoopInner {
        async fn execute(&self, _call: &ToolCall) -> ToolOutcome {
            outcome(true, "inner".into(), None)
        }
    }

    fn coordinator_fixture() -> (tempfile::TempDir, Arc<JournalCoordinator>) {
        let dir = tempfile::tempdir().unwrap();
        let coordinator = Arc::new(
            JournalCoordinator::open(dir.path().join("s.jsonl")).expect("coordinator opens"),
        );
        (dir, coordinator)
    }

    #[tokio::test]
    async fn tool_search_double_guard_and_invalid_params() {
        let (_dir, coordinator) = coordinator_fixture();
        let registry = Arc::new(Mutex::new(McpRegistry::new()));
        let inner = NoopInner;
        let executor = McpSessionToolExecutor::new(Some(registry), coordinator, "s".into(), &inner);
        // Forced invocation with nothing deferred: TYPED refusal (§3.2
        // double-guard), never an ok result.
        let outcome = executor
            .execute(&ToolCall {
                id: "c1".into(),
                name: "tool_search".into(),
                arguments: serde_json::json!({"query": "anything"}),
            })
            .await;
        assert!(!outcome.ok);
        assert_eq!(outcome.error_kind, Some(NanoErrorKind::UnknownTool));
        // Empty query ⇒ InvalidParams.
        let outcome = executor
            .execute(&ToolCall {
                id: "c2".into(),
                name: "tool_search".into(),
                arguments: serde_json::json!({"query": "   "}),
            })
            .await;
        assert!(!outcome.ok);
        assert_eq!(outcome.error_kind, Some(NanoErrorKind::InvalidParams));
    }

    #[tokio::test]
    async fn resources_args_validation_and_delegation() {
        let (_dir, coordinator) = coordinator_fixture();
        let registry = Arc::new(Mutex::new(McpRegistry::new()));
        let inner = NoopInner;
        let executor = McpSessionToolExecutor::new(Some(registry), coordinator, "s".into(), &inner);
        let outcome = executor
            .execute(&ToolCall {
                id: "c3".into(),
                name: "mcp_read_resource".into(),
                arguments: serde_json::json!({"server": "fs"}),
            })
            .await;
        assert!(!outcome.ok);
        assert_eq!(outcome.error_kind, Some(NanoErrorKind::InvalidParams));
        // Non-MCP-session names delegate.
        let outcome = executor
            .execute(&ToolCall {
                id: "c4".into(),
                name: "fs_read".into(),
                arguments: serde_json::json!({}),
            })
            .await;
        assert!(outcome.ok);
        assert_eq!(outcome.output, "inner");
    }

    #[tokio::test]
    async fn no_registry_is_typed_unavailable_never_silent() {
        let (_dir, coordinator) = coordinator_fixture();
        let inner = NoopInner;
        let executor = McpSessionToolExecutor::new(None, coordinator, "s".into(), &inner);
        let outcome = executor
            .execute(&ToolCall {
                id: "c5".into(),
                name: "mcp_list_resources".into(),
                arguments: serde_json::json!({"server": "fs"}),
            })
            .await;
        assert!(!outcome.ok);
        assert_eq!(outcome.error_kind, Some(NanoErrorKind::UnknownTool));
        let _ = ApprovalDecision::Approve; // keep the import honest
    }
}
