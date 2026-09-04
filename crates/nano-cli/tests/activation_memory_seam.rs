use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use nano_activation::admission::{AdmissionGate, AdmittedToken};
use nano_activation::authority::{AuthorityKey, AuthoritySnapshot, IssuerAuthority};
use nano_activation::journal::AuthorityRecord;
use nano_activation::policy::{BudgetLimits, EffectiveCapability, PolicyCeiling};
use nano_activation::receipt::{ArtifactIdentity, ReceiptError, ReceiptSigner};
use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::{acp_mode, activation::SharedAdmission};
use nano_memory::{ConfiguredAgents, FactWrite, MemoryPolicy, MemoryStore, SourceTrust};
use nano_model::types::{
    ContentBlock, ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage,
};
use nano_protocol::{acp::AvailableModel, permission_mode::PermissionMode};
use nano_session::{NanoErrorKind, Op, OpEnvelope, read_journal};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
const PROJECT: &str = "project-a";
const PRINCIPAL: &str = "main";
const MEMORY_NEEDLE: &str = "runner continuity needle";

static ROUTING: nano_cli::auto_routing::RoutingConfig = nano_cli::auto_routing::RoutingConfig {
    auto_opt_in: false,
    configured_default: None,
    tools_probe: false,
};

const EXEC_ROUTING: nano_cli::exec_mode::ExecRouting = nano_cli::exec_mode::ExecRouting {
    mode: nano_session::RoutingMode::ImplicitAliasPassthrough,
    reference: String::new(),
    tools_probe: false,
};

struct TestReceiptSigner(SigningKey);

impl ReceiptSigner for TestReceiptSigner {
    fn key_id(&self) -> &str {
        "receipt-test-1"
    }

    fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    fn preflight(&self) -> Result<(), ReceiptError> {
        Ok(())
    }

    fn sign(&self, message: &[u8]) -> Result<[u8; 64], ReceiptError> {
        Ok(self.0.sign(message).to_bytes())
    }
}

struct ActivationFixture {
    gate: SharedAdmission,
    issuer: SigningKey,
}

impl ActivationFixture {
    fn new(home: &Path) -> Self {
        let issuer = SigningKey::from_bytes(&[1; 32]);
        let receipt = SigningKey::from_bytes(&[9; 32]);
        bootstrap_authority(home, &issuer, &receipt);
        let deadline = wire_time(chrono::Utc::now() + chrono::Duration::hours(1));
        let gate = SharedAdmission::from_gate(
            AdmissionGate::open_enabled(
                home,
                Box::new(TestReceiptSigner(receipt)),
                PolicyCeiling {
                    capabilities: [EffectiveCapability::FilesystemRead].into(),
                    controls: BTreeSet::new(),
                    budgets: BudgetLimits {
                        max_turns: 20,
                        max_tool_calls: 20,
                        max_input_tokens: 100_000,
                        max_output_tokens: 100_000,
                        max_cost_microcents: 100_000,
                        wall_clock_ms: 60_000,
                    },
                    deadline_utc: deadline,
                },
                artifact(),
            )
            .unwrap(),
        );
        Self { gate, issuer }
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(
        &self,
        id: u64,
        method: &str,
        activation_id: &str,
        strategy: &str,
        fallback: &str,
        session_id: Option<&str>,
        fingerprint: Option<&str>,
        cwd: &Path,
    ) -> Vec<u8> {
        let now = chrono::Utc::now();
        let issued = now - chrono::Duration::seconds(5);
        let not_before = now - chrono::Duration::seconds(10);
        let not_after = now + chrono::Duration::minutes(10);
        let mut carrier = serde_json::json!({
            "activation_id": activation_id,
            "alg": "Ed25519",
            "budgets": {
                "max_cost_microcents": 100_000,
                "max_input_tokens": 100_000,
                "max_output_tokens": 100_000,
                "max_tool_calls": 20,
                "max_turns": 20,
                "wall_clock_ms": 60_000
            },
            "capabilities": ["filesystem.read"],
            "continuity": {
                "fallback": fallback,
                "resume_fingerprint": fingerprint,
                "strategy": strategy
            },
            "controls": [],
            "deadline": wire_time(not_after),
            "idempotency_key": format!("idem-{activation_id}"),
            "issued_at": wire_time(issued),
            "issuer_id": "desktop",
            "key_id": "desktop-key-1",
            "nonce": format!("nonce-{activation_id}"),
            "not_after": wire_time(not_after),
            "not_before": wire_time(not_before),
            "principal_id": PRINCIPAL,
            "product_subject_id": "subject-a",
            "project_id": PROJECT,
            "schema": "wayland.nano.activation/v1",
            "session_id": session_id
        });
        let payload = serde_jcs::to_vec(&carrier).unwrap();
        let mut message = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
        message.extend_from_slice(&payload);
        carrier.as_object_mut().unwrap().insert(
            "signature".into(),
            serde_json::json!(URL_SAFE_NO_PAD.encode(self.issuer.sign(&message).to_bytes())),
        );
        serde_jcs::to_vec(&serde_json::json!({
            "id": id,
            "jsonrpc": "2.0",
            "method": method,
            "params": {
                "cwd": cwd,
                "mcpServers": [],
                "sessionId": session_id,
                "_meta": {"waylandNanoActivation": carrier}
            }
        }))
        .unwrap()
    }

    fn admit(&self, frame: &[u8]) -> AdmittedToken {
        let admission = self
            .gate
            .admit_transport(frame, &nano_cli::activation::now_utc())
            .unwrap();
        let nano_cli::activation::TransportAdmission::Activation(token) = admission else {
            panic!("activation token")
        };
        *token
    }
}

fn wire_time(time: chrono::DateTime<chrono::Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn artifact() -> ArtifactIdentity {
    ArtifactIdentity {
        source_commit_sha: "0".repeat(40),
        cargo_lock_sha256: "1".repeat(64),
        executable_sha256: "2".repeat(64),
    }
}

fn bootstrap_authority(home: &Path, issuer: &SigningKey, receipt: &SigningKey) {
    let mut keys = BTreeMap::new();
    keys.insert(
        "desktop-key-1".into(),
        AuthorityKey {
            public_key: issuer.verifying_key().to_bytes(),
            epoch: 1,
            revoked: false,
        },
    );
    let mut snapshot = AuthoritySnapshot::empty("root", [7; 32])
        .with_service_keys(receipt.verifying_key().to_bytes(), [8; 32]);
    snapshot.issuers.insert(
        "desktop".into(),
        IssuerAuthority {
            subject_id: "subject-a".into(),
            principal_id: PRINCIPAL.into(),
            epoch: 1,
            revoked: false,
            keys,
            projects: BTreeSet::from([PROJECT.into()]),
        },
    );
    let root = home.join("activation");
    std::fs::create_dir_all(&root).unwrap();
    let bootstrap_receipt = nano_activation::admin::sign_bootstrap_receipt(
        &snapshot,
        &TestReceiptSigner(SigningKey::from_bytes(&receipt.to_bytes())),
    )
    .unwrap();
    let mut bytes = serde_jcs::to_vec(&AuthorityRecord::Bootstrap {
        sequence: 1,
        snapshot,
    })
    .unwrap();
    bytes.push(b'\n');
    bytes.extend_from_slice(
        &serde_jcs::to_vec(&AuthorityRecord::BootstrapReceipt {
            sequence: 2,
            receipt: String::from_utf8(bootstrap_receipt).unwrap(),
        })
        .unwrap(),
    );
    bytes.push(b'\n');
    std::fs::write(root.join("authority.jsonl"), bytes).unwrap();
    let enablement = nano_activation::enablement::EnablementCommand {
        operation_id: "enable-runner-memory-tests".into(),
        enabled: true,
        artifact: artifact(),
        admin_epoch: 1,
        issuer_epoch: 1,
        grant_epoch: 1,
        revocation_epoch: 1,
        not_after: wire_time(chrono::Utc::now() + chrono::Duration::hours(1)),
    };
    let mut enablement_journal = serde_jcs::to_vec(&enablement).unwrap();
    enablement_journal.push(b'\n');
    std::fs::write(root.join("enablement.jsonl"), enablement_journal).unwrap();
    std::fs::write(root.join("enablement.anchor"), enablement.digest()).unwrap();
}

fn fingerprint(token: &AdmittedToken) -> String {
    Sha256::digest(token.receipt().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn configured() -> ConfiguredAgents {
    ConfiguredAgents::try_from_ids(std::iter::empty()).unwrap()
}

fn write_enabled_policy(home: &Path) -> nano_cli::memory_policy::ResolvedMemoryPolicy {
    std::fs::write(
        home.join("memory-policy.toml"),
        "enabled = true\nwrite = \"SessionAndProject\"\nread_scope = \"SessionAndProject\"\nembedding_backend = \"HashedLocal\"\ndeletion = \"Never\"\nmin_tier = \"ModelInference\"\n\n[retention]\nepisodes = 100\nfacts = 100\nbytes = 1048576\n",
    )
    .unwrap();
    nano_cli::memory_policy::resolve(home).unwrap()
}

fn seed_memory(home: &Path, marker: &str) {
    let resolved = write_enabled_policy(home);
    let mut store = MemoryStore::open(
        home,
        &home.join("memory.jsonl"),
        resolved.policy().clone(),
        PRINCIPAL,
        resolved.configured_agents().clone(),
    )
    .unwrap();
    store
        .write_fact(FactWrite {
            id: format!("seed-{marker}"),
            subject: "runtime".into(),
            predicate: "remembers".into(),
            object: format!("{MEMORY_NEEDLE} {marker}"),
            confidence: 1.0,
            source_episode: None,
            valid_from: "2026-09-03T00:00:00Z".into(),
            valid_to: None,
            source_trust: SourceTrust::User,
            project: PROJECT.into(),
            agent_id: PRINCIPAL.into(),
        })
        .unwrap();
    drop(store);
    std::fs::write(home.join("memory/legacy.md"), b"legacy-state\n").unwrap();
}

fn text_response(text: &str) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        events: vec![
            ModelEvent::TextDelta(text.into()),
            ModelEvent::Done {
                stop_reason: "end_turn".into(),
            },
        ],
        usage: Usage::default(),
        stop_reason: "end_turn".into(),
        model: None,
    })
}

fn tool_response(name: &str, arguments: serde_json::Value) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        events: vec![
            ModelEvent::ToolCallComplete(ToolCall {
                id: format!("call-{name}"),
                name: name.into(),
                arguments,
            }),
            ModelEvent::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        usage: Usage::default(),
        stop_reason: "tool_calls".into(),
        model: None,
    })
}

#[derive(Debug, Clone)]
struct CaptureModel {
    responses: Arc<Mutex<VecDeque<Result<ModelResponse, ModelError>>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    calls: Arc<AtomicUsize>,
}

impl CaptureModel {
    fn scripted(responses: Vec<Result<ModelResponse, ModelError>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl ModelDriver for CaptureModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| text_response("done"))
    }
}

#[derive(Debug, Clone, Default)]
struct CaptureTools {
    calls: Arc<Mutex<Vec<ToolCall>>>,
}

#[async_trait::async_trait]
impl ToolExecutor for CaptureTools {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        self.calls.lock().unwrap().push(call.clone());
        ToolOutcome {
            ok: true,
            output: format!("tool output from {}", call.name),
            progress: ProgressSignals {
                new_information: true,
                ..Default::default()
            },
            error_kind: None,
        }
    }
}

fn workspace_policy() -> nano_core::permissions::FileSystemSandboxPolicy {
    nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy()
}

struct ChannelReader {
    rx: std::sync::mpsc::Receiver<String>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = {
            let available = self.fill_buf()?;
            let n = available.len().min(out.len());
            out[..n].copy_from_slice(&available[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for ChannelReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        while self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(line) => {
                    self.buf = line.into_bytes();
                    self.pos = 0;
                }
                Err(_) => return Ok(&[]),
            }
        }
        Ok(&self.buf[self.pos..])
    }

    fn consume(&mut self, amount: usize) {
        self.pos += amount;
    }
}

struct ChannelWriter {
    tx: std::sync::mpsc::Sender<String>,
    buf: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        while let Some(end) = self.buf.iter().position(|byte| *byte == b'\n') {
            let line = self.buf.drain(..=end).collect::<Vec<_>>();
            self.tx
                .send(String::from_utf8_lossy(&line).into_owned())
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

struct AcpHarness {
    input: Option<std::sync::mpsc::Sender<String>>,
    output: std::sync::mpsc::Receiver<String>,
    thread: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
}

impl AcpHarness {
    fn spawn(
        home: &Path,
        workspace: &Path,
        gate: SharedAdmission,
        model: CaptureModel,
        tools: CaptureTools,
        fail_policy_append: bool,
    ) -> Self {
        ensure_test_flux_key();
        let (input_tx, input_rx) = std::sync::mpsc::channel();
        let (output_tx, output_rx) = std::sync::mpsc::channel();
        let home = home.to_path_buf();
        let workspace = workspace.to_path_buf();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let sessions = home.join("sessions");
                std::fs::create_dir_all(&sessions).unwrap();
                let memory = acp_mode::MemoryHostConfig {
                    dir: home.join("legacy-memory"),
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                    policy: nano_cli::memory_policy::resolve(&home).unwrap(),
                };
                let sandbox_probe = || true;
                let router = nano_cli::provider_router::ProviderRouter::default();
                let vision = nano_model::vision_catalog::VisionCatalog::vendored().unwrap();
                let hooks = nano_hooks::HookEngine::empty();
                let failer = move || fail_policy_append;
                let models = vec![AvailableModel {
                    id: "flux-auto".into(),
                    name: "Flux Auto".into(),
                }];
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions,
                    default_model: "flux-auto",
                    available_models: &models,
                    env_mcp_specs: &[],
                    catalog: &[],
                    window_override: None,
                    limit_override: None,
                    reasoning_effort: None,
                    verbosity: None,
                    sandbox_probe: &sandbox_probe,
                    router: &router,
                    journal_append_failer: fail_policy_append
                        .then_some(&failer as &(dyn Fn() -> bool + Send + Sync)),
                    memory: &memory,
                    cron_home: None,
                    search: None,
                    search_meter: None,
                    pricing: None,
                    budget_cap: None,
                    vision_catalog: &vision,
                    attachment_home: &home,
                    hooks: &hooks,
                    routing: &ROUTING,
                };
                acp_mode::serve_admitted(
                    ChannelReader {
                        rx: input_rx,
                        buf: Vec::new(),
                        pos: 0,
                    },
                    ChannelWriter {
                        tx: output_tx,
                        buf: Vec::new(),
                    },
                    &config,
                    move |_| model.clone(),
                    move |root, _, _, _, _, _| {
                        assert_eq!(root, workspace);
                        (tools.clone(), workspace_policy())
                    },
                    gate,
                )
                .await
            })
        });
        Self {
            input: Some(input_tx),
            output: output_rx,
            thread: Some(thread),
        }
    }

    fn request(&self, id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.send(
            serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            }))
            .unwrap(),
        );
        self.response(id)
    }

    fn signed_request(&self, id: u64, frame: Vec<u8>) -> serde_json::Value {
        self.send(frame);
        self.response(id)
    }

    fn send(&self, frame: Vec<u8>) {
        self.input
            .as_ref()
            .unwrap()
            .send(format!("{}\n", String::from_utf8(frame).unwrap()))
            .unwrap();
    }

    fn response(&self, id: u64) -> serde_json::Value {
        loop {
            let line = self.output.recv_timeout(TIMEOUT).unwrap();
            let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
            if frame.get("method").and_then(serde_json::Value::as_str)
                == Some("session/request_permission")
            {
                let permission_id = frame["id"].as_u64().unwrap();
                self.send(
                    serde_json::to_vec(&serde_json::json!({
                        "jsonrpc":"2.0",
                        "id": permission_id,
                        "result":{"outcome":{"outcome":"selected","optionId":"allow"}}
                    }))
                    .unwrap(),
                );
                continue;
            }
            if frame.get("id").and_then(serde_json::Value::as_u64) == Some(id) {
                return frame;
            }
        }
    }

    fn shutdown(mut self) {
        drop(self.input.take());
        let exit = self.thread.take().unwrap().join().unwrap().unwrap();
        assert_eq!(exit, 0);
    }
}

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn exec_params(
    prompt: &str,
    resume: Option<nano_cli::exec_mode::ResumeTarget>,
) -> nano_cli::exec_mode::ExecParams {
    nano_cli::exec_mode::ExecParams {
        prompt: prompt.into(),
        mode: PermissionMode::Default,
        resume,
        output_last_message: None,
        goal: None,
        model: None,
        auto: false,
        activation_request: None,
    }
}

async fn run_exec(
    home: &Path,
    workspace: &Path,
    params: &nano_cli::exec_mode::ExecParams,
    gate: SharedAdmission,
    token: AdmittedToken,
    model: CaptureModel,
    tools: CaptureTools,
) -> (i32, Vec<serde_json::Value>) {
    run_exec_with_hook(
        home,
        workspace,
        params,
        gate,
        token,
        model,
        tools,
        || Ok(()),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_exec_with_hook<FB>(
    home: &Path,
    workspace: &Path,
    params: &nano_cli::exec_mode::ExecParams,
    gate: SharedAdmission,
    token: AdmittedToken,
    model: CaptureModel,
    tools: CaptureTools,
    before_memory_policy: FB,
) -> (i32, Vec<serde_json::Value>)
where
    FB: FnOnce() -> std::io::Result<()>,
{
    let output = Arc::new(Mutex::new(Vec::new()));
    let ladder = model.clone();
    let exit = nano_cli::exec_run::run_exec_with_bootstrap_hook(
        &home.join("sessions"),
        home,
        workspace,
        params,
        Some((gate, token)),
        "fake-model",
        move || model.clone(),
        move || ladder.clone(),
        move |_, _| (tools.clone(), workspace_policy()),
        false,
        false,
        &[],
        &EXEC_ROUTING,
        SharedWriter(output.clone()),
        before_memory_policy,
    )
    .await;
    let frames = String::from_utf8(output.lock().unwrap().clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (exit, frames)
}

#[allow(clippy::too_many_arguments)]
async fn run_protocol_host<FB>(
    home: &Path,
    workspace: &Path,
    gate: SharedAdmission,
    token: AdmittedToken,
    model: CaptureModel,
    tools: CaptureTools,
    input: &str,
    before_memory_policy: FB,
) -> (nano_cli::host_mode::HostExit, String)
where
    FB: FnOnce() -> std::io::Result<()>,
{
    let mut reader = std::io::Cursor::new(input.as_bytes());
    let mut output = Vec::new();
    let exit = nano_cli::host_mode::run_admitted_with(
        home,
        workspace,
        gate,
        token,
        false,
        model,
        tools,
        false,
        Vec::new(),
        &mut reader,
        &mut output,
        before_memory_policy,
    )
    .await
    .unwrap();
    (exit, String::from_utf8(output).unwrap())
}

fn scripted_surface_probe() -> Vec<Result<ModelResponse, ModelError>> {
    vec![
        tool_response("memory_recall", serde_json::json!({"query": MEMORY_NEEDLE})),
        tool_response("fs_read", serde_json::json!({"path": "probe.txt"})),
        tool_response("memory_list", serde_json::json!({})),
        text_response("done"),
    ]
}

fn assert_request_surface(requests: &[ModelRequest], marker: &str, expect_context_recall: bool) {
    assert!(!requests.is_empty(), "model request for {marker}");
    let first = &requests[0];
    let names = first
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"memory_recall"), "{marker}: {names:?}");
    assert!(names.contains(&"memory_propose"), "{marker}: {names:?}");
    for legacy in ["memory_list", "memory_read", "memory_save", "memory_delete"] {
        assert!(
            !names.contains(&legacy),
            "{marker}: legacy {legacy} advertised"
        );
    }
    let first_text = request_text(first);
    assert_eq!(
        first_text.contains(MEMORY_NEEDLE),
        expect_context_recall,
        "{marker}: unexpected automatic recall: {first_text}"
    );
    assert!(
        requests.iter().any(|request| {
            request.messages.iter().any(|message| {
                message.content.iter().any(|block| {
                    matches!(block, ContentBlock::ToolResult { content, is_error: false, .. } if content.contains(MEMORY_NEEDLE))
                })
            })
        }),
        "{marker}: explicit memory_recall never returned the seeded row"
    );
    assert!(
        requests.iter().any(|request| {
            request.messages.iter().any(|message| {
                message.content.iter().any(|block| {
                    matches!(block, ContentBlock::ToolResult { content, is_error: true, .. } if content.contains("legacy memory tool is unavailable"))
                })
            })
        }),
        "{marker}: forced legacy call was not typed as a tool error"
    );
}

fn request_text(request: &ModelRequest) -> String {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_start_segment(journal: &Path, session_id: &str, expected_begin_ordinal: usize) {
    let rows = read_journal(journal).unwrap().envelopes;
    let begins = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            matches!(&row.op, Op::SessionBegin { session_id: durable, .. } if durable == session_id)
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let begin = begins[expected_begin_ordinal];
    let next_begin = begins
        .get(expected_begin_ordinal + 1)
        .copied()
        .unwrap_or(rows.len());
    let segment = &rows[begin..next_begin];
    assert!(matches!(segment[0].op, Op::SessionBegin { .. }));
    assert!(matches!(segment[1].op, Op::MemoryPolicyResolved {
        project: Some(ref project), agent_id: Some(ref agent), session_id: Some(ref runtime), ..
    } if project == PROJECT && agent == PRINCIPAL && runtime == session_id));
    assert_eq!(
        segment
            .iter()
            .filter(|row| matches!(row.op, Op::MemoryPolicyResolved { .. }))
            .count(),
        1,
        "exactly one policy row in persistent-start segment"
    );
    let first_effect = segment
        .iter()
        .position(|row| {
            matches!(
                row.op,
                Op::TurnBegin { .. }
                    | Op::ToolCall { .. }
                    | Op::ToolResult { .. }
                    | Op::MemoryWriteFact { .. }
                    | Op::MemoryWriteDecision { .. }
                    | Op::MemoryWriteEpisode { .. }
                    | Op::MemoryWriteProcedure { .. }
            )
        })
        .unwrap_or(segment.len());
    assert!(first_effect > 1, "policy row must precede first effect");
}

fn assert_host_ingest(home: &Path) {
    let rows = read_journal(&home.join("memory.jsonl")).unwrap().envelopes;
    assert!(
        rows.iter()
            .any(|row| matches!(&row.op, Op::MemoryWriteEpisode {
        source_trust, project, agent_id, content, source, source_product, valid_from, valid_to, ..
    } if source_trust == "User"
        && project == PROJECT
        && agent_id == PRINCIPAL
        && content.contains("probe")
        && source == "host"
        && source_product == "wayland-nano"
        && !valid_from.is_empty()
        && valid_to.is_none()))
    );
    assert!(
        rows.iter()
            .any(|row| matches!(&row.op, Op::MemoryWriteEpisode {
        source_trust, project, agent_id, content, source, source_product, valid_from, valid_to, ..
    } if source_trust == "ToolOutput"
        && project == PROJECT
        && agent_id == PRINCIPAL
        && content.contains("tool output from fs_read")
        && source == "host"
        && source_product == "wayland-nano"
        && !valid_from.is_empty()
        && valid_to.is_none()))
    );
    assert_eq!(
        std::fs::read(home.join("memory/legacy.md")).unwrap(),
        b"legacy-state\n"
    );
}

fn ensure_test_flux_key() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var("FLUX_API_KEY").is_err() {
            unsafe { std::env::set_var("FLUX_API_KEY", "sk-test-harness-never-networked") };
        }
    });
}

#[test]
fn attributed_policy_record_round_trips_and_legacy_shape_stays_readable() {
    let op = nano_cli::memory_seam::policy_audit_op(
        &MemoryPolicy::default(),
        PROJECT,
        PRINCIPAL,
        "session-real",
    );
    let encoded = serde_json::to_string(&OpEnvelope::new("policy-1", "now", op)).unwrap();
    let decoded: OpEnvelope = serde_json::from_str(&encoded).unwrap();
    assert!(matches!(decoded.op, Op::MemoryPolicyResolved {
        project: Some(ref project), agent_id: Some(ref agent), session_id: Some(ref session), ..
    } if project == PROJECT && agent == PRINCIPAL && session == "session-real"));
    let legacy = r#"{"v":1,"id":"legacy","ts":"now","op":{"type":"memory_policy_resolved","enabled":true,"write":"SessionAndProject","read_scope":"SessionAndProject","episode_cap":1,"fact_cap":1,"byte_cap":1,"deletion":"Never","min_tier":"ModelInference"}}"#;
    let decoded: OpEnvelope = serde_json::from_str(legacy).unwrap();
    assert!(matches!(
        decoded.op,
        Op::MemoryPolicyResolved {
            project: None,
            agent_id: None,
            session_id: None,
            ..
        }
    ));
}

#[test]
fn store_open_validates_but_does_not_emit_a_duplicate_policy_audit() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("memory.jsonl");
    let _store = MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &journal,
        MemoryPolicy::default(),
        PRINCIPAL,
        configured(),
    )
    .unwrap();
    assert!(
        read_journal(&journal)
            .unwrap()
            .envelopes
            .iter()
            .all(|row| !matches!(row.op, Op::MemoryPolicyResolved { .. }))
    );
}

#[test]
fn memory_seam_definitions_expose_only_recall_and_mediated_propose() {
    let names = nano_cli::memory_seam::tool_definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["memory_recall", "memory_propose"]);
    for legacy in ["memory_list", "memory_read", "memory_save", "memory_delete"] {
        assert!(!names.iter().any(|name| name == legacy));
    }
}

#[test]
fn acp_session_new_runs_real_memory_surface_and_host_ingest() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    seed_memory(home, "acp-new");
    let activation = ActivationFixture::new(home);
    let model = CaptureModel::scripted(scripted_surface_probe());
    let tools = CaptureTools::default();
    let harness = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        model.clone(),
        tools.clone(),
        false,
    );
    let response = harness.signed_request(
        1,
        activation.frame(
            1,
            "session/new",
            "acp-new",
            "memory_recall",
            "none",
            None,
            None,
            &workspace,
        ),
    );
    let session_id = response["result"]["sessionId"].as_str().unwrap().to_owned();
    let prompt = harness.request(
        2,
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type":"text","text":"acp-new probe"}]
        }),
    );
    assert!(prompt.get("result").is_some(), "{prompt}");
    harness.shutdown();

    assert_request_surface(&model.requests.lock().unwrap(), "acp-new", true);
    assert_eq!(
        tools
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["fs_read"]
    );
    assert_start_segment(
        &home.join("sessions").join(format!("{session_id}.jsonl")),
        &session_id,
        0,
    );
    assert_host_ingest(home);
}

#[test]
fn acp_session_load_runs_real_memory_surface_after_resume_begin() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    write_enabled_policy(home);
    let activation = ActivationFixture::new(home);

    let first = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        CaptureModel::scripted(vec![]),
        CaptureTools::default(),
        false,
    );
    let first_token_frame = activation.frame(
        10,
        "session/new",
        "acp-load-origin",
        "fresh",
        "none",
        None,
        None,
        &workspace,
    );
    let admitted = activation.admit(&first_token_frame);
    let expected_fingerprint = fingerprint(&admitted);
    let created = first.signed_request(10, first_token_frame);
    let session_id = created["result"]["sessionId"].as_str().unwrap().to_owned();
    first.shutdown();
    seed_memory(home, "acp-load");

    let model = CaptureModel::scripted(scripted_surface_probe());
    let tools = CaptureTools::default();
    let resumed = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        model.clone(),
        tools.clone(),
        false,
    );
    let loaded = resumed.signed_request(
        11,
        activation.frame(
            11,
            "session/load",
            "acp-load-resume",
            "session_resume",
            "none",
            Some(&session_id),
            Some(&expected_fingerprint),
            &workspace,
        ),
    );
    assert!(loaded.get("result").is_some(), "{loaded}");
    let prompt = resumed.request(
        12,
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type":"text","text":"acp-load probe"}]
        }),
    );
    assert!(prompt.get("result").is_some(), "{prompt}");
    resumed.shutdown();

    assert_request_surface(&model.requests.lock().unwrap(), "acp-load", false);
    assert_eq!(
        tools
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["fs_read"]
    );
    assert_start_segment(
        &home.join("sessions").join(format!("{session_id}.jsonl")),
        &session_id,
        1,
    );
    assert_host_ingest(home);
}

#[test]
fn acp_policy_append_failure_stops_before_model_tool_or_memory_effect() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    write_enabled_policy(home);
    let activation = ActivationFixture::new(home);
    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let tools = CaptureTools::default();
    let harness = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        model.clone(),
        tools.clone(),
        true,
    );
    let response = harness.signed_request(
        20,
        activation.frame(
            20,
            "session/new",
            "acp-append-fail",
            "memory_recall",
            "none",
            None,
            None,
            &workspace,
        ),
    );
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"],
        serde_json::json!(NanoErrorKind::JournalUnavailable)
    );
    harness.shutdown();
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!home.join("memory/memory.db").exists());
    assert!(!home.join("memory.jsonl").exists());
    let journal = std::fs::read_dir(home.join("sessions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .unwrap();
    let rows = read_journal(&journal).unwrap().envelopes;
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].op, Op::SessionBegin { .. }));
}

#[test]
fn acp_load_policy_append_failure_stops_before_model_tool_or_memory_effect() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let first = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        CaptureModel::scripted(vec![]),
        CaptureTools::default(),
        false,
    );
    let first_frame = activation.frame(
        25,
        "session/new",
        "acp-load-fail-origin",
        "fresh",
        "none",
        None,
        None,
        &workspace,
    );
    let first_token = activation.admit(&first_frame);
    let expected_fingerprint = fingerprint(&first_token);
    let created = first.signed_request(25, first_frame);
    let session_id = created["result"]["sessionId"].as_str().unwrap().to_owned();
    first.shutdown();
    write_enabled_policy(home);

    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let tools = CaptureTools::default();
    let resumed = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        model.clone(),
        tools.clone(),
        true,
    );
    let response = resumed.signed_request(
        26,
        activation.frame(
            26,
            "session/load",
            "acp-load-append-fail",
            "session_resume",
            "none",
            Some(&session_id),
            Some(&expected_fingerprint),
            &workspace,
        ),
    );
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"],
        serde_json::json!(NanoErrorKind::JournalUnavailable)
    );
    resumed.shutdown();
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!home.join("memory/memory.db").exists());
    assert!(!home.join("memory.jsonl").exists());
    let rows = read_journal(&home.join("sessions").join(format!("{session_id}.jsonl")))
        .unwrap()
        .envelopes;
    let last_begin = rows
        .iter()
        .rposition(|row| matches!(row.op, Op::SessionBegin { .. }))
        .unwrap();
    assert_eq!(last_begin, rows.len() - 1);
}

#[test]
fn acp_runtime_none_refuses_before_model_or_memory_write() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let tools = CaptureTools::default();
    let harness = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        model.clone(),
        tools.clone(),
        false,
    );
    let created = harness.signed_request(
        27,
        activation.frame(
            27,
            "session/new",
            "acp-runtime-none",
            "memory_recall",
            "none",
            None,
            None,
            &workspace,
        ),
    );
    let session_id = created["result"]["sessionId"].as_str().unwrap().to_owned();
    let response = harness.request(
        28,
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type":"text","text":"runtime none"}]
        }),
    );
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"],
        serde_json::json!(NanoErrorKind::ActivationContinuityNotEnabled)
    );
    harness.shutdown();
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(tools.calls.lock().unwrap().is_empty());
    let session_rows = read_journal(&home.join("sessions").join(format!("{session_id}.jsonl")))
        .unwrap()
        .envelopes;
    assert_eq!(
        session_rows
            .iter()
            .filter(|row| matches!(row.op, Op::MemoryWriteReceipt { .. }))
            .count(),
        0
    );
    let memory_rows = read_journal(&home.join("memory.jsonl")).unwrap().envelopes;
    assert!(memory_rows.is_empty());
}

#[test]
fn acp_runtime_fresh_journals_once_and_calls_model() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let model = CaptureModel::scripted(vec![text_response("continued fresh")]);
    let tools = CaptureTools::default();
    let harness = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        model.clone(),
        tools.clone(),
        false,
    );
    let created = harness.signed_request(
        29,
        activation.frame(
            29,
            "session/new",
            "acp-runtime-fresh",
            "memory_recall",
            "fresh",
            None,
            None,
            &workspace,
        ),
    );
    let session_id = created["result"]["sessionId"].as_str().unwrap().to_owned();
    let response = harness.request(
        30,
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type":"text","text":"runtime fresh"}]
        }),
    );
    assert!(response.get("result").is_some(), "{response}");
    harness.shutdown();
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert!(tools.calls.lock().unwrap().is_empty());
    let rows = read_journal(&home.join("sessions").join(format!("{session_id}.jsonl")))
        .unwrap()
        .envelopes;
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row.op, Op::MemoryWriteReceipt { .. }))
            .count(),
        1
    );
}

#[test]
fn acp_runtime_fallback_append_failure_is_loud_before_model() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let tools = CaptureTools::default();
    let harness = AcpHarness::spawn(
        home,
        &workspace,
        activation.gate.clone(),
        model.clone(),
        tools.clone(),
        false,
    );
    let created = harness.signed_request(
        31,
        activation.frame(
            31,
            "session/new",
            "acp-runtime-fallback-append-fail",
            "memory_recall",
            "fresh",
            None,
            None,
            &workspace,
        ),
    );
    let session_id = created["result"]["sessionId"].as_str().unwrap().to_owned();
    let journal = home.join("sessions").join(format!("{session_id}.jsonl"));
    std::fs::remove_file(&journal).unwrap();
    let response = harness.request(
        32,
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type":"text","text":"fallback append failure"}]
        }),
    );
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"],
        serde_json::json!(NanoErrorKind::JournalUnavailable)
    );
    harness.shutdown();
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(tools.calls.lock().unwrap().is_empty());
    let memory_rows = read_journal(&home.join("memory.jsonl")).unwrap().envelopes;
    assert!(memory_rows.is_empty());
}

#[tokio::test]
async fn exec_fresh_runs_real_memory_surface_and_host_ingest() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    seed_memory(home, "exec-fresh");
    let activation = ActivationFixture::new(home);
    let frame = activation.frame(
        30,
        "session/new",
        "exec-fresh",
        "fresh",
        "none",
        None,
        None,
        &workspace,
    );
    let token = activation.admit(&frame);
    let model = CaptureModel::scripted(scripted_surface_probe());
    let tools = CaptureTools::default();
    let (exit, events) = run_exec(
        home,
        &workspace,
        &exec_params("exec-fresh probe", None),
        activation.gate.clone(),
        token,
        model.clone(),
        tools.clone(),
    )
    .await;
    assert_eq!(exit, 0, "{events:?}");
    let session_id = events[0]["session_id"].as_str().unwrap();
    assert_request_surface(&model.requests.lock().unwrap(), "exec-fresh", false);
    assert_eq!(
        tools
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["fs_read"]
    );
    assert_start_segment(
        &home.join("sessions").join(format!("{session_id}.jsonl")),
        session_id,
        0,
    );
    assert_host_ingest(home);
}

#[tokio::test]
async fn exec_policy_append_failure_stops_before_model_tool_or_memory_effect() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    write_enabled_policy(home);
    let activation = ActivationFixture::new(home);
    let frame = activation.frame(
        35,
        "session/new",
        "exec-append-fail",
        "memory_recall",
        "none",
        None,
        None,
        &workspace,
    );
    let token = activation.admit(&frame);
    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let tools = CaptureTools::default();
    let (exit, _events) = run_exec_with_hook(
        home,
        &workspace,
        &exec_params("must not run", None),
        activation.gate.clone(),
        token,
        model.clone(),
        tools.clone(),
        || Err(std::io::Error::other("injected policy append failure")),
    )
    .await;
    assert_eq!(exit, 2);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!home.join("memory/memory.db").exists());
    assert!(!home.join("memory.jsonl").exists());
    let journal = std::fs::read_dir(home.join("sessions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .unwrap();
    let rows = read_journal(&journal).unwrap().envelopes;
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].op, Op::SessionBegin { .. }));
}

#[tokio::test]
async fn exec_runtime_none_refuses_before_model_effect() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let frame = activation.frame(
        36,
        "session/new",
        "exec-runtime-none",
        "memory_recall",
        "none",
        None,
        None,
        &workspace,
    );
    let token = activation.admit(&frame);
    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let (exit, _events) = run_exec(
        home,
        &workspace,
        &exec_params("runtime none", None),
        activation.gate.clone(),
        token,
        model.clone(),
        CaptureTools::default(),
    )
    .await;
    assert_eq!(exit, 2);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exec_runtime_fresh_journals_once_and_calls_model() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let frame = activation.frame(
        37,
        "session/new",
        "exec-runtime-fresh",
        "memory_recall",
        "fresh",
        None,
        None,
        &workspace,
    );
    let token = activation.admit(&frame);
    let model = CaptureModel::scripted(vec![text_response("continued fresh")]);
    let (exit, events) = run_exec(
        home,
        &workspace,
        &exec_params("runtime fresh", None),
        activation.gate.clone(),
        token,
        model.clone(),
        CaptureTools::default(),
    )
    .await;
    assert_eq!(exit, 0, "{events:?}");
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    let session_id = events[0]["session_id"].as_str().unwrap();
    let rows = read_journal(&home.join("sessions").join(format!("{session_id}.jsonl")))
        .unwrap()
        .envelopes;
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row.op, Op::MemoryWriteReceipt { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn exec_resume_runs_real_memory_surface_after_resume_begin() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    write_enabled_policy(home);
    let activation = ActivationFixture::new(home);
    let first_frame = activation.frame(
        40,
        "session/new",
        "exec-resume-origin",
        "fresh",
        "none",
        None,
        None,
        &workspace,
    );
    let first_token = activation.admit(&first_frame);
    let expected_fingerprint = fingerprint(&first_token);
    let (first_exit, first_events) = run_exec(
        home,
        &workspace,
        &exec_params("origin", None),
        activation.gate.clone(),
        first_token,
        CaptureModel::scripted(vec![text_response("origin done")]),
        CaptureTools::default(),
    )
    .await;
    assert_eq!(first_exit, 0);
    let session_id = first_events[0]["session_id"].as_str().unwrap().to_owned();
    seed_memory(home, "exec-resume");
    let resume_frame = activation.frame(
        41,
        "session/load",
        "exec-resume",
        "session_resume",
        "none",
        Some(&session_id),
        Some(&expected_fingerprint),
        &workspace,
    );
    let resume_token = activation.admit(&resume_frame);
    let model = CaptureModel::scripted(scripted_surface_probe());
    let tools = CaptureTools::default();
    let (exit, events) = run_exec(
        home,
        &workspace,
        &exec_params(
            "exec-resume probe",
            Some(nano_cli::exec_mode::ResumeTarget::Id(session_id.clone())),
        ),
        activation.gate.clone(),
        resume_token,
        model.clone(),
        tools.clone(),
    )
    .await;
    assert_eq!(exit, 0, "{events:?}");
    assert_request_surface(&model.requests.lock().unwrap(), "exec-resume", false);
    assert_eq!(
        tools
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["fs_read"]
    );
    assert_start_segment(
        &home.join("sessions").join(format!("{session_id}.jsonl")),
        &session_id,
        1,
    );
    assert_host_ingest(home);
}

#[tokio::test]
async fn exec_resume_policy_append_failure_stops_before_model_tool_or_memory_effect() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let first_frame = activation.frame(
        45,
        "session/new",
        "exec-resume-fail-origin",
        "fresh",
        "none",
        None,
        None,
        &workspace,
    );
    let first_token = activation.admit(&first_frame);
    let expected_fingerprint = fingerprint(&first_token);
    let (first_exit, first_events) = run_exec(
        home,
        &workspace,
        &exec_params("origin", None),
        activation.gate.clone(),
        first_token,
        CaptureModel::scripted(vec![text_response("origin done")]),
        CaptureTools::default(),
    )
    .await;
    assert_eq!(first_exit, 0);
    let session_id = first_events[0]["session_id"].as_str().unwrap().to_owned();
    write_enabled_policy(home);
    let resume_frame = activation.frame(
        46,
        "session/load",
        "exec-resume-append-fail",
        "session_resume",
        "none",
        Some(&session_id),
        Some(&expected_fingerprint),
        &workspace,
    );
    let resume_token = activation.admit(&resume_frame);
    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let tools = CaptureTools::default();
    let (exit, _events) = run_exec_with_hook(
        home,
        &workspace,
        &exec_params(
            "must not run",
            Some(nano_cli::exec_mode::ResumeTarget::Id(session_id.clone())),
        ),
        activation.gate.clone(),
        resume_token,
        model.clone(),
        tools.clone(),
        || Err(std::io::Error::other("injected policy append failure")),
    )
    .await;
    assert_eq!(exit, 2);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!home.join("memory/memory.db").exists());
    assert!(!home.join("memory.jsonl").exists());
    let rows = read_journal(&home.join("sessions").join(format!("{session_id}.jsonl")))
        .unwrap()
        .envelopes;
    let last_begin = rows
        .iter()
        .rposition(|row| matches!(row.op, Op::SessionBegin { .. }))
        .unwrap();
    assert_eq!(last_begin, rows.len() - 1);
}

#[tokio::test]
async fn protocol_host_runs_real_loop_memory_surface_and_host_ingest() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    seed_memory(home, "protocol-host");
    let activation = ActivationFixture::new(home);
    let frame = activation.frame(
        50,
        "session/new",
        "protocol-host",
        "memory_recall",
        "none",
        None,
        None,
        &workspace,
    );
    let token = activation.admit(&frame);
    let model = CaptureModel::scripted(scripted_surface_probe());
    let tools = CaptureTools::default();
    let (exit, output) = run_protocol_host(
        home,
        &workspace,
        activation.gate.clone(),
        token,
        model.clone(),
        tools.clone(),
        "{\"type\":\"message\",\"msg_id\":\"m1\",\"content\":\"protocol-host probe\"}\n",
        || Ok(()),
    )
    .await;
    assert_eq!(exit, nano_cli::host_mode::HostExit::StdinClosed);
    assert!(output.contains("\"type\":\"ready\""), "{output}");
    assert!(output.contains("\"type\":\"stream_end\""), "{output}");
    assert_request_surface(&model.requests.lock().unwrap(), "protocol-host", true);
    assert_eq!(
        tools
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        ["fs_read"]
    );
    assert_start_segment(
        &home.join("sessions/protocol-host.jsonl"),
        "protocol-host",
        0,
    );
    assert_host_ingest(home);
}

#[tokio::test]
async fn protocol_host_policy_append_failure_stops_before_loop_model_tool_or_memory_effect() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    write_enabled_policy(home);
    let activation = ActivationFixture::new(home);
    let frame = activation.frame(
        51,
        "session/new",
        "protocol-host-fail",
        "memory_recall",
        "none",
        None,
        None,
        &workspace,
    );
    let token = activation.admit(&frame);
    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let tools = CaptureTools::default();
    let (exit, output) = run_protocol_host(
        home,
        &workspace,
        activation.gate.clone(),
        token,
        model.clone(),
        tools.clone(),
        "{\"type\":\"message\",\"msg_id\":\"m1\",\"content\":\"must not run\"}\n",
        || Err(std::io::Error::other("injected policy append failure")),
    )
    .await;
    assert!(matches!(exit, nano_cli::host_mode::HostExit::Fatal(_)));
    assert!(output.is_empty(), "host loop must not emit ready: {output}");
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(tools.calls.lock().unwrap().is_empty());
    assert!(!home.join("memory/memory.db").exists());
    assert!(!home.join("memory.jsonl").exists());
    let rows = read_journal(&home.join("sessions/protocol-host.jsonl"))
        .unwrap()
        .envelopes;
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].op, Op::SessionBegin { .. }));
}

#[tokio::test]
async fn protocol_host_runtime_none_refuses_before_model_effect() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let frame = activation.frame(
        52,
        "session/new",
        "protocol-host-runtime-none",
        "memory_recall",
        "none",
        None,
        None,
        &workspace,
    );
    let token = activation.admit(&frame);
    let model = CaptureModel::scripted(vec![text_response("must not run")]);
    let (exit, output) = run_protocol_host(
        home,
        &workspace,
        activation.gate.clone(),
        token,
        model.clone(),
        CaptureTools::default(),
        "{\"type\":\"message\",\"msg_id\":\"m1\",\"content\":\"runtime none\"}\n",
        || Ok(()),
    )
    .await;
    assert_eq!(exit, nano_cli::host_mode::HostExit::StdinClosed);
    assert!(
        output.contains("\"code\":\"continuity_not_enabled\""),
        "{output}"
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn protocol_host_runtime_fresh_journals_once_and_calls_model() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let activation = ActivationFixture::new(home);
    let frame = activation.frame(
        53,
        "session/new",
        "protocol-host-runtime-fresh",
        "memory_recall",
        "fresh",
        None,
        None,
        &workspace,
    );
    let token = activation.admit(&frame);
    let model = CaptureModel::scripted(vec![text_response("continued fresh")]);
    let (exit, output) = run_protocol_host(
        home,
        &workspace,
        activation.gate.clone(),
        token,
        model.clone(),
        CaptureTools::default(),
        "{\"type\":\"message\",\"msg_id\":\"m1\",\"content\":\"runtime fresh\"}\n",
        || Ok(()),
    )
    .await;
    assert_eq!(exit, nano_cli::host_mode::HostExit::StdinClosed);
    assert!(output.contains("continued fresh"), "{output}");
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    let rows = read_journal(&home.join("sessions/protocol-host.jsonl"))
        .unwrap()
        .envelopes;
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row.op, Op::MemoryWriteReceipt { .. }))
            .count(),
        1
    );
}
