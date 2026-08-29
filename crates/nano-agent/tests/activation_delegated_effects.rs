use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use nano_activation::{
    admission::AdmissionGate,
    authority::{AuthorityKey, AuthoritySnapshot, IssuerAuthority},
    enablement::{EnablementCommand, EnablementFault, EnablementStore},
    journal::AuthorityRecord,
    policy::{BudgetLimits, EffectiveCapability, EffectiveControl, PolicyCeiling},
    receipt::{ArtifactIdentity, ReceiptError, ReceiptSigner},
};
use nano_agent::{
    mcp::{
        DelegatedEffectAuthority, McpRegistry, McpServerSpec, McpToolExecutor, SpecSource,
        Transport,
    },
    tasks::{TaskRegistry, TaskToolExecutor},
    turn::{ModelDriver, ToolExecutor, ToolOutcome},
};
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone)]
struct TestReceiptSigner(SigningKey);

impl ReceiptSigner for TestReceiptSigner {
    fn key_id(&self) -> &str {
        "receipt"
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

#[derive(Debug)]
struct Noop;

#[async_trait]
impl ToolExecutor for Noop {
    async fn execute(&self, _: &ToolCall) -> ToolOutcome {
        panic!("delegated call must not fall through")
    }
}

#[derive(Debug)]
struct CompleteDriver;

#[async_trait]
impl ModelDriver for CompleteDriver {
    async fn complete(&self, _: &ModelRequest) -> Result<ModelResponse, ModelError> {
        Ok(ModelResponse {
            events: vec![
                ModelEvent::TextDelta("done".into()),
                ModelEvent::Done {
                    stop_reason: "stop".into(),
                },
            ],
            model: None,
            usage: Usage::default(),
            stop_reason: "stop".into(),
        })
    }
}

#[tokio::test]
async fn mcp_ambiguous_dispatch_is_not_replayed_after_rebuild() {
    let fixture = Fixture::new();
    fixture.enable();
    let marker = fixture.root.path().join("mcp-count.txt");
    let registry = Arc::new(std::sync::Mutex::new(fake_mcp_registry(&marker)));
    let authority = fixture.authority().with_fault_after_dispatch();
    let executor =
        McpToolExecutor::from_shared(registry.clone(), &Noop).with_activation_authority(authority);
    let call = ToolCall {
        id: "mcp-call-1".into(),
        name: "mcp__fake__echo".into(),
        arguments: json!({"text":"ping"}),
    };
    assert!(!executor.execute(&call).await.ok);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "1");
    drop(executor);
    let rebuilt = McpToolExecutor::from_shared(registry, &Noop)
        .with_activation_authority(fixture.authority());
    assert!(!rebuilt.execute(&call).await.ok);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "1");
    let journal = std::fs::read_to_string(fixture.home().join("activation/effects.jsonl")).unwrap();
    assert!(journal.contains("\"tool\":\"mcp.invoke\""));
    assert!(journal.contains("\"state\":\"unknown_outcome\""));
}

#[tokio::test]
async fn mcp_default_off_refuses_before_remote_oracle_changes() {
    let fixture = Fixture::new();
    let marker = fixture.root.path().join("mcp-disabled-count.txt");
    let registry = fake_mcp_registry(&marker);
    let executor =
        McpToolExecutor::new(registry, &Noop).with_activation_authority(fixture.authority());
    let outcome = executor
        .execute(&ToolCall {
            id: "mcp-disabled".into(),
            name: "mcp__fake__echo".into(),
            arguments: json!({}),
        })
        .await;
    assert!(!outcome.ok);
    assert!(!marker.exists(), "wire oracle must remain untouched");
}

#[tokio::test]
async fn mcp_control_is_rechecked_immediately_before_remote_dispatch() {
    let fixture = Fixture::new();
    fixture.enable();
    fixture.cancel();
    let marker = fixture.root.path().join("mcp-cancelled-count.txt");
    let registry = fake_mcp_registry(&marker);
    let executor =
        McpToolExecutor::new(registry, &Noop).with_activation_authority(fixture.authority());
    let outcome = executor
        .execute(&ToolCall {
            id: "mcp-cancelled".into(),
            name: "mcp__fake__echo".into(),
            arguments: json!({}),
        })
        .await;
    assert!(!outcome.ok);
    assert!(
        !marker.exists(),
        "cancelled activation must not reach the wire"
    );
}

#[tokio::test]
async fn task_spawn_uses_downward_intent_and_never_duplicates_after_ambiguity() {
    let fixture = Fixture::new();
    fixture.enable();
    let workspace = fixture.root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("input.txt"), "input").unwrap();
    let factory: Arc<dyn Fn() -> Result<Arc<dyn ModelDriver>, String> + Send + Sync> =
        Arc::new(|| Ok(Arc::new(CompleteDriver)));
    let registry = Arc::new(
        TaskRegistry::new(fixture.home(), &workspace, "mock".into(), factory.clone())
            .with_activation_authority(fixture.authority().with_fault_after_dispatch()),
    );
    let executor = TaskToolExecutor::new(registry.clone(), &Noop);
    let call = ToolCall {
        id: "task-call-1".into(),
        name: "task_spawn".into(),
        arguments: json!({"prompt":"finish once","label":"bounded"}),
    };
    assert!(!executor.execute(&call).await.ok);
    assert_eq!(task_dir_count(fixture.home()), 1);
    drop(executor);
    drop(registry);
    let rebuilt_registry = Arc::new(
        TaskRegistry::new(fixture.home(), &workspace, "mock".into(), factory)
            .with_activation_authority(fixture.authority()),
    );
    let rebuilt = TaskToolExecutor::new(rebuilt_registry, &Noop);
    assert!(!rebuilt.execute(&call).await.ok);
    assert_eq!(task_dir_count(fixture.home()), 1);
    let journal = std::fs::read_to_string(fixture.home().join("activation/effects.jsonl")).unwrap();
    assert!(journal.contains("\"tool\":\"task.spawn\""));
    assert!(journal.contains("\"state\":\"unknown_outcome\""));
}

#[tokio::test]
async fn task_default_off_refuses_before_workspace_or_thread_effect() {
    let fixture = Fixture::new();
    let workspace = fixture.root.path().join("workspace-disabled");
    std::fs::create_dir_all(&workspace).unwrap();
    let factory: Arc<dyn Fn() -> Result<Arc<dyn ModelDriver>, String> + Send + Sync> =
        Arc::new(|| Ok(Arc::new(CompleteDriver)));
    let registry = Arc::new(
        TaskRegistry::new(fixture.home(), &workspace, "mock".into(), factory)
            .with_activation_authority(fixture.authority()),
    );
    let executor = TaskToolExecutor::new(registry, &Noop);
    let outcome = executor
        .execute(&ToolCall {
            id: "task-disabled".into(),
            name: "task_spawn".into(),
            arguments: json!({"prompt":"must not start"}),
        })
        .await;
    assert!(!outcome.ok);
    assert_eq!(task_dir_count(fixture.home()), 0);
}

fn task_dir_count(home: &std::path::Path) -> usize {
    std::fs::read_dir(home.join("tasks"))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

fn fake_mcp_registry(marker: &std::path::Path) -> McpRegistry {
    #[cfg(windows)]
    let (command, args) = {
        let script = r#"
$reader = [System.Console]::In
while ($true) {
  $line = $reader.ReadLine(); if ($null -eq $line) { break }
  $obj = $line | ConvertFrom-Json
  if ($obj.method -eq "initialize") { Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"protocolVersion`":`"2025-03-26`",`"capabilities`":{},`"serverInfo`":{`"name`":`"fake`",`"version`":`"0`"}}}") }
  elseif ($obj.method -eq "tools/list") { Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"tools`":[{`"name`":`"echo`",`"description`":`"echoes`"}]}}") }
  elseif ($obj.method -eq "tools/call") {
    $count = 0; if (Test-Path "MARKER") { $count = [int](Get-Content "MARKER") }
    [System.IO.File]::WriteAllText("MARKER", [string]($count + 1))
    Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"content`":`"pong`",`"isError`":false}}")
  }
}
"#
        .replace("MARKER", &marker.display().to_string());
        (
            "powershell.exe".to_owned(),
            vec!["-NoProfile".into(), "-Command".into(), script],
        )
    };
    #[cfg(unix)]
    let (command, args) = {
        let script = r#"
while IFS= read -r line; do
 id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
 case "$line" in
  *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}}\n' "$id" ;;
  *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echoes"}]}}\n' "$id" ;;
  *'"tools/call"'*) n=0; test -f 'MARKER' && n=$(cat 'MARKER'); n=$((n+1)); printf '%s' "$n" > 'MARKER'; printf '{"jsonrpc":"2.0","id":%s,"result":{"content":"pong","isError":false}}\n' "$id" ;;
 esac
done
"#
        .replace("MARKER", &marker.display().to_string());
        ("sh".to_owned(), vec!["-c".into(), script])
    };
    let mut registry = McpRegistry::new();
    registry
        .register(McpServerSpec {
            name: "fake".into(),
            transport: Transport::Stdio {
                command,
                args,
                env: vec![],
            },
            source: SpecSource::Config,
        })
        .unwrap();
    registry
}

struct Fixture {
    root: tempfile::TempDir,
    token: nano_activation::admission::AdmittedToken,
    root_key: SigningKey,
    issuer_key: SigningKey,
    receipt_key: SigningKey,
    artifact: ArtifactIdentity,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let issuer = SigningKey::from_bytes(&[1; 32]);
        let root_key = SigningKey::from_bytes(&[7; 32]);
        let receipt = SigningKey::from_bytes(&[9; 32]);
        bootstrap(root.path(), &issuer, &root_key, &receipt);
        let artifact = artifact();
        let mut gate = AdmissionGate::open(
            root.path(),
            Box::new(TestReceiptSigner(receipt.clone())),
            ceiling(),
            artifact.clone(),
        )
        .unwrap();
        let token = gate
            .admit_raw(&frame(&issuer), "2026-08-30T10:00:00Z", None)
            .unwrap();
        Self {
            root,
            token,
            root_key,
            issuer_key: issuer,
            receipt_key: receipt,
            artifact,
        }
    }

    fn home(&self) -> &std::path::Path {
        self.root.path()
    }

    fn authority(&self) -> DelegatedEffectAuthority {
        DelegatedEffectAuthority::new(
            self.token.clone(),
            self.home(),
            self.artifact.clone(),
            [1; 4],
            "2026-08-30T10:00:00Z",
        )
    }

    fn enable(&self) {
        let store = EnablementStore::open(self.home()).unwrap();
        let command = EnablementCommand {
            operation_id: "enable-1".into(),
            enabled: true,
            artifact: self.artifact.clone(),
            admin_epoch: 1,
            issuer_epoch: 1,
            grant_epoch: 1,
            revocation_epoch: 1,
            not_after: "2026-08-30T10:05:00Z".into(),
        };
        store
            .apply_signed(
                &admin(&self.root_key, &store, &command),
                &command,
                "2026-08-30T10:00:00Z",
                EnablementFault::None,
            )
            .unwrap();
    }

    fn cancel(&self) {
        let mut gate = AdmissionGate::open(
            self.home(),
            Box::new(TestReceiptSigner(self.receipt_key.clone())),
            ceiling(),
            self.artifact.clone(),
        )
        .unwrap();
        gate.apply_control(&control(&self.issuer_key), "2026-08-30T10:00:01Z")
            .unwrap();
    }
}

fn bootstrap(home: &std::path::Path, issuer: &SigningKey, root: &SigningKey, receipt: &SigningKey) {
    let mut keys = BTreeMap::new();
    keys.insert(
        "key".into(),
        AuthorityKey {
            public_key: issuer.verifying_key().to_bytes(),
            epoch: 1,
            revoked: false,
        },
    );
    let mut projects = BTreeSet::new();
    projects.insert("project".into());
    let mut snapshot = AuthoritySnapshot::empty("root", root.verifying_key().to_bytes())
        .with_service_keys(receipt.verifying_key().to_bytes(), [8; 32]);
    snapshot.issuers.insert(
        "desktop".into(),
        IssuerAuthority {
            subject_id: "subject".into(),
            principal_id: "main".into(),
            epoch: 1,
            revoked: false,
            keys,
            projects,
        },
    );
    let dir = home.join("activation");
    std::fs::create_dir_all(&dir).unwrap();
    let mut bytes = serde_jcs::to_vec(&AuthorityRecord::Bootstrap {
        sequence: 1,
        snapshot,
    })
    .unwrap();
    bytes.push(b'\n');
    std::fs::write(dir.join("authority.jsonl"), bytes).unwrap();
}

fn artifact() -> ArtifactIdentity {
    ArtifactIdentity {
        source_commit_sha: "0".repeat(40),
        cargo_lock_sha256: "1".repeat(64),
        executable_sha256: "2".repeat(64),
    }
}

fn ceiling() -> PolicyCeiling {
    PolicyCeiling {
        capabilities: [
            EffectiveCapability::McpInvoke,
            EffectiveCapability::TaskSpawn,
        ]
        .into(),
        controls: [EffectiveControl::Cancel].into(),
        budgets: BudgetLimits {
            max_turns: 4,
            max_tool_calls: 4,
            max_input_tokens: 100,
            max_output_tokens: 100,
            max_cost_microcents: 100,
            wall_clock_ms: 1_000,
        },
        deadline_utc: "2026-08-30T10:10:00Z".into(),
    }
}

fn frame(key: &SigningKey) -> Vec<u8> {
    let mut carrier = json!({"activation_id":"activation-1","alg":"Ed25519","budgets":{"max_cost_microcents":100,"max_input_tokens":100,"max_output_tokens":100,"max_tool_calls":4,"max_turns":4,"wall_clock_ms":1000},"capabilities":["mcp.invoke","task.spawn"],"continuity":{"fallback":"none","resume_fingerprint":null,"strategy":"fresh"},"controls":["cancel"],"deadline":"2026-08-30T10:05:00Z","idempotency_key":"idem-1","issued_at":"2026-08-30T09:59:59Z","issuer_id":"desktop","key_id":"key","nonce":"nonce-1","not_after":"2026-08-30T10:05:00Z","not_before":"2026-08-30T09:59:00Z","principal_id":"main","product_subject_id":"subject","project_id":"project","schema":"wayland.nano.activation/v1","session_id":"session-1"});
    let payload = serde_jcs::to_vec(&carrier).unwrap();
    let mut message = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
    message.extend(payload);
    carrier.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
    );
    serde_jcs::to_vec(&json!({"id":1,"jsonrpc":"2.0","method":"session/new","params":{"_meta":{"waylandNanoActivation":carrier}}})).unwrap()
}

fn control(key: &SigningKey) -> Vec<u8> {
    let mut value = json!({"activation_id":"activation-1","alg":"Ed25519","control":"cancel","issued_at":"2026-08-30T10:00:01Z","issuer_id":"desktop","key_id":"key","nonce":"cancel-nonce","not_after":"2026-08-30T10:05:00Z","principal_id":"main","project_id":"project","schema":"wayland.nano.control/v1","session_id":"session-1"});
    let payload = serde_jcs::to_vec(&value).unwrap();
    let mut message = b"WAYLAND-NANO-CONTROL\0v1\0".to_vec();
    message.extend(payload);
    value.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
    );
    serde_jcs::to_vec(&value).unwrap()
}

fn admin(key: &SigningKey, store: &EnablementStore, command: &EnablementCommand) -> Vec<u8> {
    let mut value = json!({"admin_epoch":1,"admin_id":"root","after_digest":command.digest(),"alg":"Ed25519","before_digest":store.state_digest().unwrap(),"issued_at":"2026-08-30T09:59:59Z","key_id":"root-key","nonce":"enable-nonce","not_after":"2026-08-30T10:05:00Z","operation":"enable_artifact","operation_id":command.operation_id,"reason":"test","schema":"wayland.nano.admin-request/v1"});
    let payload = serde_jcs::to_vec(&value).unwrap();
    let mut message = b"WAYLAND-NANO-ADMIN\0v1\0".to_vec();
    message.extend(payload);
    value.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
    );
    serde_jcs::to_vec(&value).unwrap()
}
