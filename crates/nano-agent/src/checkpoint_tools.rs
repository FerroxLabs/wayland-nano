//! Session executor wrapper for workspace checkpoint tools.

use crate::turn::{ToolExecutor, ToolOutcome};
use nano_checkpoints::CheckpointStore;
use nano_model::types::ToolCall;
use nano_session::{JournalCoordinator, NanoErrorKind};
use std::sync::Arc;

pub struct CheckpointToolExecutor<'a> {
    store: Arc<CheckpointStore>,
    coordinator: Arc<JournalCoordinator>,
    session_id: String,
    inner: &'a dyn ToolExecutor,
}

impl std::fmt::Debug for CheckpointToolExecutor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckpointToolExecutor")
            .field("session_id", &self.session_id)
            .field("workspace_key", &self.store.workspace_key())
            .finish_non_exhaustive()
    }
}

impl<'a> CheckpointToolExecutor<'a> {
    pub fn new(
        store: Arc<CheckpointStore>,
        coordinator: Arc<JournalCoordinator>,
        session_id: String,
        inner: &'a dyn ToolExecutor,
    ) -> Self {
        Self {
            store,
            coordinator,
            session_id,
            inner,
        }
    }

    fn failure(kind: NanoErrorKind, message: &str) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            output: message.to_string(),
            progress: Default::default(),
            error_kind: Some(kind),
        }
    }

    fn success(value: serde_json::Value) -> ToolOutcome {
        ToolOutcome {
            ok: true,
            output: value.to_string(),
            progress: Default::default(),
            error_kind: None,
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for CheckpointToolExecutor<'_> {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        match call.name.as_str() {
            "checkpoint_create" => {
                let label = call.arguments.get("label").and_then(|v| v.as_str());
                match self
                    .store
                    .create(&self.coordinator, &self.session_id, label)
                {
                    Ok(result) => Self::success(serde_json::json!({
                        "checkpoint_id": result.checkpoint.id,
                        "parent": result.checkpoint.parent,
                        "label": result.checkpoint.label,
                        "file_count": result.checkpoint.file_count,
                        "total_bytes": result.checkpoint.total_bytes,
                        "tree_digest": result.checkpoint.tree_digest,
                        "evicted": result.evicted,
                        "warnings": result.warnings,
                    })),
                    Err(err) => Self::failure(err.kind, "checkpoint create unavailable"),
                }
            }
            "checkpoint_list" => match self.store.list() {
                Ok(checkpoints) => Self::success(serde_json::json!({"checkpoints": checkpoints})),
                Err(err) => Self::failure(err.kind, "checkpoint list unavailable"),
            },
            "checkpoint_restore" => {
                let Some(id) = call.arguments.get("checkpoint_id").and_then(|v| v.as_str()) else {
                    return Self::failure(NanoErrorKind::MissingArgs, "checkpoint_id is required");
                };
                match self.store.restore(&self.coordinator, &self.session_id, id) {
                    Ok(result) => Self::success(serde_json::json!({
                        "checkpoint_id": result.checkpoint_id,
                        "safety_checkpoint_id": result.safety_checkpoint_id,
                        "skipped_sensitive": result.skipped_sensitive,
                    })),
                    Err(err) => Self::failure(err.kind, "checkpoint restore unavailable"),
                }
            }
            _ => self.inner.execute(call).await,
        }
    }

    /// F-P3-5: delegate the mid-turn hydration refresh through the wrapper
    /// chain to the MCP-merged executor.
    fn current_mcp_tool_definitions(&self) -> Option<Vec<nano_model::types::ToolDefinition>> {
        self.inner.current_mcp_tool_definitions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::TempDir;

    #[derive(Debug)]
    struct Inner;
    #[async_trait::async_trait]
    impl ToolExecutor for Inner {
        async fn execute(&self, _call: &ToolCall) -> ToolOutcome {
            CheckpointToolExecutor::success(serde_json::json!({"delegated": true}))
        }
    }

    fn git(root: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }

    #[tokio::test]
    async fn services_names_and_delegates_unknown_names() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        git(&workspace, &["init"]);
        fs::write(workspace.join("a.txt"), "a").unwrap();
        git(&workspace, &["add", "a.txt"]);
        let coordinator = Arc::new(JournalCoordinator::open(temp.path().join("j.jsonl")).unwrap());
        let store = Arc::new(CheckpointStore::open(temp.path().join("home"), &workspace).unwrap());
        let inner = Inner;
        let executor = CheckpointToolExecutor::new(store, coordinator, "s".into(), &inner);
        let created = executor
            .execute(&ToolCall {
                id: "1".into(),
                name: "checkpoint_create".into(),
                arguments: serde_json::json!({}),
            })
            .await;
        assert!(created.ok);
        let delegated = executor
            .execute(&ToolCall {
                id: "2".into(),
                name: "fs_read".into(),
                arguments: serde_json::json!({}),
            })
            .await;
        assert!(delegated.ok);
        assert!(delegated.output.contains("delegated"));
    }
}
