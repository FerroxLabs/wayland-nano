//! Durable authority wrapper for the real local tool-effect boundary.

use crate::loop_protection::ProgressSignals;
use crate::turn::{LiveImageToolResult, ToolExecutor, ToolOutcome};
use nano_activation::{
    admission::{AdmittedToken, validate_live_effect_authority},
    policy::EffectiveCapability,
    receipt::ArtifactIdentity,
};
use nano_model::types::{ToolCall, ToolDefinition};
use nano_session::{FileLock, NanoErrorKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectFault {
    None,
    AfterDispatch,
}

#[derive(Debug)]
pub struct ActivationEffectExecutor<T> {
    inner: T,
    token: AdmittedToken,
    home: PathBuf,
    artifact: ArtifactIdentity,
    epochs: [u64; 4],
    now_utc: String,
    live_clock: bool,
    fault: EffectFault,
}

impl<T> ActivationEffectExecutor<T> {
    pub fn new(
        inner: T,
        token: AdmittedToken,
        home: &Path,
        artifact: ArtifactIdentity,
        epochs: [u64; 4],
        now_utc: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            token,
            home: home.into(),
            artifact,
            epochs,
            now_utc: now_utc.into(),
            live_clock: false,
            fault: EffectFault::None,
        }
    }
    pub fn new_live(
        inner: T,
        token: AdmittedToken,
        home: &Path,
        artifact: ArtifactIdentity,
        epochs: [u64; 4],
    ) -> Self {
        Self {
            inner,
            token,
            home: home.into(),
            artifact,
            epochs,
            now_utc: String::new(),
            live_clock: true,
            fault: EffectFault::None,
        }
    }
    pub fn with_fault(mut self, fault: EffectFault) -> Self {
        self.fault = fault;
        self
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum EffectRecord {
    Intent {
        effect_id: String,
        activation_id: String,
        tool: String,
    },
    Result {
        effect_id: String,
        digest: String,
    },
    UnknownOutcome {
        effect_id: String,
    },
}

#[async_trait::async_trait]
impl<T: ToolExecutor> ToolExecutor for ActivationEffectExecutor<T> {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        self.execute_cancellable(call, None).await
    }
    async fn execute_cancellable(
        &self,
        call: &ToolCall,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> ToolOutcome {
        let Some(capability) = capability_for(&call.name) else {
            return refused("unmapped activation effect");
        };
        if !self.token.policy().capabilities().contains(&capability) {
            return refused("activation capability denied");
        }
        let now = if self.live_clock {
            current_utc()
        } else {
            self.now_utc.clone()
        };
        if validate_live_effect_authority(
            &self.token,
            &self.home,
            &self.artifact,
            self.epochs,
            &now,
        )
        .is_err()
        {
            return refused("activation authority is not current");
        }
        let id = effect_id(self.token.activation_id(), call);
        let _lock = match FileLock::try_acquire(&self.home.join("activation/effects.lock")) {
            Ok(v) => v,
            Err(_) => return refused("activation effect ledger unavailable"),
        };
        let path = self.home.join("activation/effects.jsonl");
        let existing = std::fs::read(&path).unwrap_or_default();
        let mut pending = false;
        let mut terminal = false;
        let mut activation_intents = 0u64;
        for line in existing.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            let Ok(record) = serde_json::from_slice::<EffectRecord>(line) else {
                return refused("activation effect ledger ambiguous");
            };
            if let EffectRecord::Intent { activation_id, .. } = &record
                && activation_id == self.token.activation_id()
            {
                activation_intents += 1;
            }
            match record {
                EffectRecord::Intent { effect_id, .. } if effect_id == id => pending = true,
                EffectRecord::Result { effect_id, .. }
                | EffectRecord::UnknownOutcome { effect_id }
                    if effect_id == id =>
                {
                    terminal = true
                }
                _ => {}
            }
        }
        if terminal {
            return refused("activation effect is already terminal");
        }
        if pending {
            let _ = append(&path, &EffectRecord::UnknownOutcome { effect_id: id });
            return refused("activation effect outcome is unknown; reconciliation required");
        }
        if activation_intents >= self.token.policy().budgets().max_tool_calls {
            return refused("activation tool-call budget exhausted");
        }
        if append(
            &path,
            &EffectRecord::Intent {
                effect_id: id.clone(),
                activation_id: self.token.activation_id().into(),
                tool: call.name.clone(),
            },
        )
        .is_err()
        {
            return refused("activation effect intent was not durable");
        }
        let outcome = self.inner.execute_cancellable(call, cancel).await;
        if self.fault == EffectFault::AfterDispatch {
            return refused("injected crash after external effect");
        }
        let digest = hex(&Sha256::digest(outcome.output.as_bytes()));
        if append(
            &path,
            &EffectRecord::Result {
                effect_id: id,
                digest,
            },
        )
        .is_err()
        {
            return refused("activation effect result was not durable");
        }
        outcome
    }
    fn take_image_result(&self, call_id: &str) -> Option<LiveImageToolResult> {
        self.inner.take_image_result(call_id)
    }
    fn image_results_backed(&self) -> bool {
        self.inner.image_results_backed()
    }
    fn current_mcp_tool_definitions(&self) -> Option<Vec<ToolDefinition>> {
        self.inner.current_mcp_tool_definitions()
    }
}

pub(crate) fn current_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    chrono::DateTime::from_timestamp(seconds as i64, 0).map_or_else(String::new, |dt| {
        dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    })
}

fn capability_for(name: &str) -> Option<EffectiveCapability> {
    match name {
        "fs_read" | "fs_list" | "search" | "repo_map" | "view_image" => {
            Some(EffectiveCapability::FilesystemRead)
        }
        "fs_write" | "fs_edit" => Some(EffectiveCapability::FilesystemWrite),
        "shell" => Some(EffectiveCapability::ShellExecute),
        "web_fetch" | "web_search" => Some(EffectiveCapability::NetworkEgress),
        "checkpoint_create" | "checkpoint_restore" => Some(EffectiveCapability::CheckpointMutate),
        _ => None,
    }
}
fn effect_id(activation: &str, call: &ToolCall) -> String {
    let canonical=serde_json::to_vec(&serde_json::json!({"activation_id":activation,"arguments":call.arguments,"call_id":call.id,"tool":call.name})).expect("effect identity");
    hex(&Sha256::digest(canonical))
}
fn append(path: &Path, record: &EffectRecord) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?
    }
    let mut bytes = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(&bytes)?;
    f.sync_data()
}
fn refused(message: &str) -> ToolOutcome {
    ToolOutcome {
        ok: false,
        output: message.into(),
        progress: ProgressSignals::default(),
        error_kind: Some(NanoErrorKind::ApprovalDenied),
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
