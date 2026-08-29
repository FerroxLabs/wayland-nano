//! C8 provider parity — integration battery over `acp_mode::serve` and the
//! routing modules (design §9):
//!
//! - payload validation (trust-boundary invariants, limits, dedup, order);
//! - the `<provider>:<model>` namespace parser (Q2);
//! - the set_model routing matrix (bare flux id, namespaced known id,
//!   unknown provider/model, hasKey advisory semantics, mid-session
//!   re-resolution, the provider_unproven gate);
//! - B2 startup semantics (deterministic initial binding; exit-2 rule);
//! - the synthetic-bearer contract (Q1b): fresh bearer routes + dispatches,
//!   expired bearer → typed retryable oauth_expired with the respawn hint;
//! - ACP-frame canary: no resolved credential ever appears in any frame.
//!
//! Env-mutating cases run under one file-wide lock and always restore the
//! process env (other test files are separate processes).

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_cli::provider_key::Credential;
use nano_cli::provider_router::{
    KIND_OAUTH_EXPIRED, KIND_PROVIDER_KEY_MISSING, KIND_PROVIDER_UNPROVEN, ProviderRouter,
};
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use nano_protocol::acp::AvailableModel;
use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Mutex, MutexGuard};

// ── env discipline ──────────────────────────────────────────────────────

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// P5: the fail-closed default routing posture for test hosts (no Auto
/// opt-in, no configured default).
static DEFAULT_ROUTING: nano_cli::auto_routing::RoutingConfig =
    nano_cli::auto_routing::RoutingConfig {
        auto_opt_in: false,
        configured_default: None,
        tools_probe: false,
    };

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Every env var these tests may touch (restored after each case).
const TOUCHED_VARS: &[&str] = &[
    "FLUX_API_KEY",
    "FLUX_TEST_KEY",
    "FLUX_API_KEY_FILE",
    "OPENAI_API_KEY",
    "XAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "NVIDIA_API_KEY",
    "WAYLAND_NANO_OAUTH_BEARER_XAI",
    "WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS",
    "WAYLAND_NANO_PROVIDERS",
];

fn clear_env() {
    for var in TOUCHED_VARS {
        unsafe { std::env::remove_var(var) };
    }
}

fn env_reader(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

// ── payload validation (pure) ───────────────────────────────────────────

#[test]
fn payload_validation_battery() {
    // Unknown provider ids are ignored — nothing advertised for them.
    let router = ProviderRouter::from_payload(Some(
        r#"[{"provider":"not-a-provider","models":["m1"],"hasKey":true}]"#,
    ))
    .expect("unknown ids are ignored, not errors");
    assert!(router.providers().is_empty());
    assert!(router.advertised_models().is_empty());

    // Entries carrying endpoint/routing fields are DROPPED (the payload can
    // never redirect an endpoint).
    let router = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"openai","models":["gpt-5.6-terra"],"hasKey":true,"base_url":"https://evil.example"},
            {"provider":"xai","models":["grok-9"],"hasKey":false}
        ]"#,
    ))
    .expect("valid entries survive");
    assert_eq!(router.providers().len(), 1);
    assert_eq!(router.providers()[0].spec.id, "xai");

    // Malformed payloads are ignored WHOLESALE with a diagnostic.
    for bad in [
        "not json",
        r#"{"provider":"openai"}"#,
        r#"[42]"#,
        r#"[{"provider":"openai","models":"gpt-5.6-terra","hasKey":true}]"#,
        r#"[{"provider":"openai","models":["m"],"hasKey":"yes"}]"#,
        r#"[{"models":["m"],"hasKey":true}]"#,
    ] {
        assert!(
            ProviderRouter::from_payload(Some(bad)).is_err(),
            "payload must be rejected wholesale: {bad}"
        );
    }

    // Limits: 32 KiB total, 64 entries, 256 models/provider.
    let oversize = format!(
        r#"[{{"provider":"openai","models":["{}"],"hasKey":true}}]"#,
        "m".repeat(33 * 1024)
    );
    assert!(ProviderRouter::from_payload(Some(&oversize)).is_err());
    let too_many_entries = format!(
        "[{}]",
        (0..65)
            .map(|_| r#"{"provider":"openai","models":["m"],"hasKey":true}"#)
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(ProviderRouter::from_payload(Some(&too_many_entries)).is_err());
    let too_many_models = format!(
        r#"[{{"provider":"openai","models":[{}],"hasKey":true}}]"#,
        (0..257)
            .map(|i| format!(r#""m{i}""#))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(ProviderRouter::from_payload(Some(&too_many_models)).is_err());
    // F-19: an overlong model id is ONE malformed entry — dropped with a
    // typed warning, no longer wholesale-fatal.
    let long_id = format!(
        r#"[{{"provider":"openai","models":["{}"],"hasKey":true}}]"#,
        "m".repeat(129)
    );
    let router = ProviderRouter::from_payload(Some(&long_id))
        .expect("a lone overlong id drops, the payload survives");
    assert_eq!(router.payload_warnings().len(), 1);
    assert!(
        router.payload_warnings()[0]
            .starts_with(nano_cli::provider_router::KIND_PAYLOAD_ENTRY_INVALID),
        "typed warning: {}",
        router.payload_warnings()[0]
    );
    // The provider entry survives with zero models (nothing advertised).
    assert!(router.advertised_models().is_empty());

    // Dedup: first occurrence wins.
    let router = ProviderRouter::from_payload(Some(
        r#"[{"provider":"openai","models":["a","b","a"],"hasKey":true}]"#,
    ))
    .expect("valid");
    assert_eq!(router.providers()[0].models, ["a", "b"]);

    // Deterministic advertisement order: catalog-table order, independent
    // of the payload's entry order.
    let fwd = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"openai","models":["gpt-5.6-terra"],"hasKey":true},
            {"provider":"xai","models":["grok-9"],"hasKey":false}
        ]"#,
    ))
    .expect("valid");
    let rev = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"xai","models":["grok-9"],"hasKey":false},
            {"provider":"openai","models":["gpt-5.6-terra"],"hasKey":true}
        ]"#,
    ))
    .expect("valid");
    let fwd_ids: Vec<String> = fwd
        .advertised_models()
        .iter()
        .map(|m| m.id.clone())
        .collect();
    let rev_ids: Vec<String> = rev
        .advertised_models()
        .iter()
        .map(|m| m.id.clone())
        .collect();
    assert_eq!(fwd_ids, rev_ids);
    // openai precedes xai in the vendored catalog order.
    assert_eq!(fwd_ids, ["openai:gpt-5.6-terra", "xai:grok-9"]);
    // Display names are human-friendly (Q2), ids stay the routing key.
    let names: Vec<String> = fwd
        .advertised_models()
        .iter()
        .map(|m| m.name.clone())
        .collect();
    assert_eq!(names, ["gpt-5.6-terra (OpenAI)", "grok-9 (xAI)"]);
}

/// F-19: ONE colon-bearing (or empty/non-string) model entry no longer
/// bricks the whole payload — the bad entry drops with a typed
/// `payload_entry_invalid` warning, the rest of the payload survives.
/// (The live-proof matrix hit this via OpenRouter's live /models list
/// carrying ids like `openai/gpt-5-mini:batch` / `:free`.) A structurally
/// malformed payload stays wholesale-fatal.
#[test]
fn f19_one_bad_model_entry_drops_the_rest_survives() {
    let router = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"openai","models":["gpt-5.6-terra","openai/gpt-5-mini:batch",""],"hasKey":true},
            {"provider":"xai","models":[42,"grok-9"],"hasKey":true}
        ]"#,
    ))
    .expect("one bad entry must not brick the payload");
    // The three malformed entries dropped; the two good ids survived.
    assert_eq!(router.payload_warnings().len(), 3);
    for warning in router.payload_warnings() {
        assert!(
            warning.starts_with(nano_cli::provider_router::KIND_PAYLOAD_ENTRY_INVALID),
            "typed warning: {warning}"
        );
    }
    let ids: Vec<String> = router
        .advertised_models()
        .iter()
        .map(|m| m.id.clone())
        .collect();
    assert_eq!(ids, ["openai:gpt-5.6-terra", "xai:grok-9"]);

    // The fully-malformed CATALOG stays fatal (fail-closed wholesale).
    let err = ProviderRouter::from_payload(Some(r#"[{"provider":"openai"}]"#))
        .expect_err("structural malformation stays wholesale-fatal");
    assert!(
        err.starts_with(nano_cli::provider_router::KIND_PAYLOAD_INVALID),
        "typed fatal diagnostic: {err}"
    );
}

// ── namespace parser (Q2) ───────────────────────────────────────────────

#[test]
fn namespace_parser_accepts_and_rejects() {
    use nano_cli::provider_router::{ModelRef, ProviderRouter};
    assert_eq!(
        ProviderRouter::parse_model_id("flux-auto").expect("bare"),
        ModelRef::Flux("flux-auto".into())
    );
    assert_eq!(
        ProviderRouter::parse_model_id("openai:gpt-5.6-terra").expect("namespaced"),
        ModelRef::Namespaced {
            provider: "openai".into(),
            model: "gpt-5.6-terra".into(),
        }
    );
    for bad in ["a:b:c", ":model", "openai:", "", ":"] {
        let err = ProviderRouter::parse_model_id(bad).expect_err("malformed must reject");
        assert_eq!(err.kind, "model_not_found", "{bad}");
    }
}

// ── set_model routing matrix (router level) ─────────────────────────────

fn parity_router() -> ProviderRouter {
    let mut router = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"openai","models":["gpt-5.6-terra"],"hasKey":false},
            {"provider":"xai","models":["grok-9"],"hasKey":true},
            {"provider":"anthropic","models":["claude-opus-4-8"],"hasKey":true},
            {"provider":"nvidia","models":["nv-model-1"],"hasKey":true}
        ]"#,
    ))
    .expect("valid payload");
    // TEST SEAM: the proof lane flips these in the vendored catalog after
    // the live compat proof; the matrix exercises the success arm now.
    // 2026-08-12: the live provider proofs flipped 14 providers (incl.
    // anthropic and openrouter) in the real catalog; `nvidia` stays
    // unproven (no key exists) and is now the unproven-gate arm.
    router.mark_proven_for_tests(&["openai", "xai"]);
    router
}

#[test]
fn set_model_routing_matrix() {
    let _guard = env_lock();
    clear_env();
    let now = 1_000u64;
    let router = parity_router();

    // Bare flux id + flux key → flux binding, bare model on the wire.
    unsafe { std::env::set_var("FLUX_API_KEY", "sk-flux-matrix") };
    let binding = router
        .resolve_binding("flux-auto", &env_reader, now)
        .expect("flux resolves");
    assert_eq!(binding.provider_id, "flux-router");
    assert_eq!(binding.model, "flux-auto");

    // Bare flux id without a flux key → typed provider_key_missing.
    clear_env();
    let err = router
        .resolve_binding("flux-auto", &env_reader, now)
        .expect_err("no flux key");
    assert_eq!(err.kind, KIND_PROVIDER_KEY_MISSING);

    // Namespaced known id with key → binding carries the catalog endpoint
    // (sole authority) and the BARE model id.
    unsafe { std::env::set_var("OPENAI_API_KEY", "sk-openai-matrix") };
    let binding = router
        .resolve_binding("openai:gpt-5.6-terra", &env_reader, now)
        .expect("openai resolves");
    assert_eq!(binding.provider_id, "openai");
    assert_eq!(binding.model, "gpt-5.6-terra");
    assert_eq!(binding.base_url, "https://api.openai.com/v1");
    assert_eq!(binding.api_path, "/chat/completions");

    // hasKey:false but a resolvable credential → SUCCEEDS (hasKey is
    // advisory only).
    assert_eq!(
        router
            .providers()
            .iter()
            .find(|p| p.spec.id == "openai")
            .map(|p| p.has_key),
        Some(false)
    );

    // hasKey:true but NO resolvable credential → typed provider_key_missing.
    clear_env();
    let err = router
        .resolve_binding("xai:grok-9", &env_reader, now)
        .expect_err("no xai credential");
    assert_eq!(err.kind, KIND_PROVIDER_KEY_MISSING);
    let frame = err.acp_response(serde_json::json!(7));
    let wire = serde_json::to_value(&frame).expect("serialize");
    assert_eq!(wire["error"]["data"]["kind"], "provider_key_missing");
    assert_eq!(wire["error"]["data"]["retryable"], false);
    // Names the env var NAME, never a value.
    let msg = wire["error"]["message"].as_str().unwrap();
    assert!(msg.contains("XAI_API_KEY"), "{msg}");

    // Unknown provider / unknown model → model_not_found.
    for id in ["nosuch:m", "openai:nosuch-model"] {
        let err = router
            .resolve_binding(id, &env_reader, now)
            .expect_err("unknown must fail");
        assert_eq!(err.kind, "model_not_found", "{id}");
    }

    // Unproven provider (no test-seam mark) WITH a credential → the typed
    // provider_unproven gate, never a silent fallback.
    unsafe { std::env::set_var("NVIDIA_API_KEY", "sk-nv-matrix") };
    let err = router
        .resolve_binding("nvidia:nv-model-1", &env_reader, now)
        .expect_err("unproven arm");
    assert_eq!(err.kind, KIND_PROVIDER_UNPROVEN);
    assert!(!err.retryable);

    // Mid-session switch re-resolution: a credential that resolves at one
    // set_model and is gone at the next fails closed.
    clear_env();
    unsafe { std::env::set_var("OPENAI_API_KEY", "sk-openai-matrix") };
    assert!(
        router
            .resolve_binding("openai:gpt-5.6-terra", &env_reader, now)
            .is_ok()
    );
    clear_env();
    let err = router
        .resolve_binding("openai:gpt-5.6-terra", &env_reader, now)
        .expect_err("vanished credential");
    assert_eq!(err.kind, KIND_PROVIDER_KEY_MISSING);
    clear_env();
}

// ── synthetic-bearer contract (Q1b) ─────────────────────────────────────

#[test]
fn bearer_contract_fresh_routes_expired_typed() {
    let _guard = env_lock();
    clear_env();
    let now = 1_000u64;
    let router = parity_router();
    let canary = format!("bearer-C8CANARY-{}", std::process::id());

    // Fresh synthetic bearer routes and binds (access token only).
    unsafe {
        std::env::set_var("WAYLAND_NANO_OAUTH_BEARER_XAI", &canary);
        std::env::set_var("WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS", "2000");
    }
    let binding = router
        .resolve_binding("xai:grok-9", &env_reader, now)
        .expect("fresh bearer routes");
    assert_eq!(
        binding.credential,
        Credential::Bearer {
            token: canary.clone(),
            expires_at_unix_secs: Some(2000),
        }
    );
    assert!(binding.check_fresh(now).is_ok());
    // The bearer registered with the sanitization boundary at resolution.
    assert!(
        !nano_egress::redact::sanitize_text(&format!("echo {canary}")).contains(&canary),
        "bearer must be redacted from error surfaces"
    );

    // Expired bearer → typed oauth_expired, RETRYABLE, respawn hint.
    unsafe { std::env::set_var("WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS", "999") };
    let err = router
        .resolve_binding("xai:grok-9", &env_reader, now)
        .expect_err("expired bearer");
    assert_eq!(err.kind, KIND_OAUTH_EXPIRED);
    assert!(err.retryable);
    let frame = err.acp_response(serde_json::json!(9));
    let wire = serde_json::to_value(&frame).expect("serialize");
    assert_eq!(wire["error"]["data"]["kind"], "oauth_expired");
    assert_eq!(wire["error"]["data"]["retryable"], true);
    assert!(
        wire["error"]["data"]["hint"]
            .as_str()
            .unwrap()
            .contains("respawn"),
        "the respawn hint is pre-wired: {wire}"
    );
    // The serialized frame never carries the bearer itself.
    let text = serde_json::to_string(&wire).unwrap();
    assert!(
        !text.contains(&canary),
        "bearer leaked into error frame: {text}"
    );
    clear_env();
}

// ── B2 startup semantics (pure level; process exit-2 below) ─────────────

#[test]
fn startup_initial_binding_is_deterministic() {
    let _guard = env_lock();
    clear_env();
    let now = 1_000u64;
    let router = parity_router();

    // Flux key only → flux-auto initial binding.
    unsafe { std::env::set_var("FLUX_API_KEY", "sk-flux-startup") };
    assert_eq!(
        router.initial_model(Some("sk-flux-startup"), &env_reader, now),
        Some("flux-auto".to_string())
    );

    // No flux key, resolvable OPENAI_API_KEY + payload → the deterministic
    // non-Flux binding (first credentialed provider in catalog order, first
    // advertised model in payload order).
    clear_env();
    unsafe { std::env::set_var("OPENAI_API_KEY", "sk-openai-startup") };
    assert_eq!(
        router.initial_model(None, &env_reader, now),
        Some("openai:gpt-5.6-terra".to_string())
    );

    // Nothing resolvable → None (caller exits 2).
    clear_env();
    assert_eq!(router.initial_model(None, &env_reader, now), None);
    // The exit-2 message names env vars only.
    let msg = router.no_credential_message();
    assert!(msg.contains("FLUX_API_KEY"));
    assert!(msg.contains("OPENAI_API_KEY"));
    clear_env();
}

/// B2 process-level: with NO resolvable credential the acp-host exits 2
/// with the generalized message (names only, no values) and never touches
/// the network or stdin.
#[test]
fn process_exits_2_with_generalized_message_when_nothing_resolves() {
    let _guard = env_lock();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-nano"));
    cmd.arg("acp-host");
    for var in TOUCHED_VARS {
        cmd.env_remove(var);
    }
    // A payload alone (no credentials) must not save the startup.
    cmd.env(
        "WAYLAND_NANO_PROVIDERS",
        r#"[{"provider":"openai","models":["gpt-5.6-terra"],"hasKey":true}]"#,
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());
    let output = cmd.output().expect("spawn acp-host");
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no usable provider credential"), "{stderr}");
    assert!(stderr.contains("FLUX_API_KEY"), "{stderr}");
    assert!(stderr.contains("OPENAI_API_KEY"), "{stderr}");
}

// ── in-process ACP host harness (channels, scripted driver) ─────────────

struct ChannelReader {
    rx: std::sync::mpsc::Receiver<String>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = {
            let avail = self.fill_buf()?;
            let n = avail.len().min(out.len());
            out[..n].copy_from_slice(&avail[..n]);
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

    fn consume(&mut self, amt: usize) {
        self.pos += amt;
    }
}

struct ChannelWriter {
    tx: std::sync::mpsc::Sender<String>,
    buf: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.tx
                .send(String::from_utf8_lossy(&line).into_owned())
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

/// Records the model id each turn dispatched on (proves the namespace is
/// stripped before the wire) and answers with a fixed text response.
#[derive(Debug, Clone, Default)]
struct CapturingDriver {
    seen_models: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl ModelDriver for CapturingDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.seen_models.lock().unwrap().push(request.model.clone());
        Ok(ModelResponse {
            events: vec![
                ModelEvent::TextDelta("ok".into()),
                ModelEvent::Done {
                    stop_reason: "stop".into(),
                },
            ],
            usage: Usage::default(),
            stop_reason: "stop".into(),
            model: None,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct MockTools;

#[async_trait::async_trait]
impl ToolExecutor for MockTools {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        ToolOutcome {
            ok: true,
            output: format!("ran {}", call.name),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

struct Host {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    seen_models: Arc<Mutex<Vec<String>>>,
}

impl Host {
    fn spawn(router: ProviderRouter, available: Vec<AvailableModel>, default_model: &str) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let sessions_dir = std::env::temp_dir().join(format!(
            "nano-c8-sessions-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&sessions_dir).expect("sessions dir");
        let driver = CapturingDriver::default();
        let seen_models = driver.seen_models.clone();
        let default_model = default_model.to_string();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let sandbox_probe = || true;
                // C5: memory store for this harness (writes off).
                let memory_config = acp_mode::MemoryHostConfig {
                    dir: sessions_dir.parent().expect("root").join("memory"),
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                };
                // P2a: lane-A vision catalog (vendored, fail-closed) + the attachment
                // store root beside the session journals (lane-B boundary use).
                let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
                    .expect("vendored vision catalog parses");
                let attachment_home = sessions_dir.parent().expect("root");
                let hooks = nano_hooks::HookEngine::empty();
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir,
                    default_model: &default_model,
                    available_models: &available,
                    env_mcp_specs: &[],
                    catalog: &[],
                    window_override: None,
                    limit_override: None,
                    sandbox_probe: &sandbox_probe,
                    router: &router,
                    journal_append_failer: None,
                    memory: &memory_config,
                    reasoning_effort: None,
                    verbosity: None,
                    cron_home: None,
                    search: None,
                    search_meter: None,
                    pricing: None,
                    budget_cap: None,
                    vision_catalog: &vision_catalog,
                    attachment_home,
                    hooks: &hooks,
                    routing: &DEFAULT_ROUTING,
                };
                acp_mode::serve_legacy_debug(
                    ChannelReader {
                        rx: in_rx,
                        buf: Vec::new(),
                        pos: 0,
                    },
                    ChannelWriter {
                        tx: out_tx,
                        buf: Vec::new(),
                    },
                    &config,
                    move |_| driver.clone(),
                    move |_, _, _, _, _, _| {
                        (
                            MockTools,
                            nano_core::permissions::PermissionProfile::workspace_write()
                                .file_system_sandbox_policy(),
                        )
                    },
                )
                .await
            })
        });
        Self {
            to_host: Some(in_tx),
            frames: out_rx,
            handle: Some(handle),
            next_id: 1,
            seen_models,
        }
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.to_host
            .as_ref()
            .expect("stdin open")
            .send(format!("{}\n", serde_json::to_string(&frame).unwrap()))
            .expect("send to host");
        // Read frames until the response to this id arrives (skip
        // session/update notifications).
        loop {
            let line = self
                .frames
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("host frame");
            let value: serde_json::Value = serde_json::from_str(&line).expect("json frame");
            if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return value;
            }
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        drop(self.to_host.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The full advertised set the router implies: one bare flux id + the
/// namespaced payload ids.
fn advertised_with(router: &ProviderRouter) -> Vec<AvailableModel> {
    let mut available = vec![AvailableModel {
        id: "flux-auto".into(),
        name: "flux-auto".into(),
    }];
    available.extend(router.advertised_models());
    available
}

/// ACP-level bearer + routing proof: a fresh synthetic bearer drives
/// session/new → set_model → prompt end-to-end; the turn dispatches on the
/// BARE model id; an expired bearer is the typed retryable error; and NO
/// frame ever carries the canary credential (B4 at the ACP boundary).
#[test]
fn acp_bearer_routes_dispatches_and_never_leaks() {
    let _guard = env_lock();
    clear_env();
    let canary = format!("bearer-C8CANARY-acp-{}", std::process::id());
    // Fresh bearer, ~1h validity.
    let future_expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + 3600;
    unsafe {
        std::env::set_var("WAYLAND_NANO_OAUTH_BEARER_XAI", &canary);
        std::env::set_var(
            "WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS",
            future_expiry.to_string(),
        );
    }
    let router = parity_router();
    let available = advertised_with(&router);
    let mut host = Host::spawn(router, available, "flux-auto");

    let new_resp = host.request(
        "session/new",
        serde_json::json!({"cwd": std::env::temp_dir()}),
    );
    let session_id = new_resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    // The namespaced model is advertised with a human-friendly name.
    let models = new_resp["result"]["models"]["availableModels"]
        .as_array()
        .expect("models block");
    assert!(
        models
            .iter()
            .any(|m| m["modelId"] == "xai:grok-9" && m["name"] == "grok-9 (xAI)"),
        "advertised models: {models:?}"
    );

    // set_model to the bearer-backed provider succeeds (hasKey:true AND a
    // fresh bearer resolves).
    let set_resp = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session_id, "modelId": "xai:grok-9"}),
    );
    assert!(
        set_resp.get("error").is_none(),
        "set_model must succeed: {set_resp}"
    );

    // A prompt dispatches on the BARE model id (namespace stripped).
    let prompt_resp = host.request(
        "session/prompt",
        serde_json::json!({"sessionId": session_id, "prompt": [{"type":"text","text":"hi"}]}),
    );
    assert!(
        prompt_resp.get("error").is_none(),
        "prompt must succeed: {prompt_resp}"
    );
    assert_eq!(
        host.seen_models.lock().unwrap().as_slice(),
        ["grok-9"],
        "the wire sees the bare model id"
    );

    // Expire the bearer mid-session: the NEXT set_model/prompt pre-fails
    // with the typed retryable oauth_expired + respawn hint — never a
    // half-authed turn.
    unsafe {
        std::env::set_var("WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS", "1");
    }
    let set_resp = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session_id, "modelId": "xai:grok-9"}),
    );
    assert_eq!(
        set_resp["error"]["data"]["kind"], "oauth_expired",
        "{set_resp}"
    );
    assert_eq!(set_resp["error"]["data"]["retryable"], true);
    assert!(
        set_resp["error"]["data"]["hint"]
            .as_str()
            .unwrap()
            .contains("respawn")
    );
    let prompt_resp = host.request(
        "session/prompt",
        serde_json::json!({"sessionId": session_id, "prompt": [{"type":"text","text":"hi"}]}),
    );
    assert_eq!(prompt_resp["error"]["data"]["kind"], "oauth_expired");

    clear_env();
}

/// ACP canary (B4): with a canary credential registered via the payload
/// provider's env var, every error path (provider_key_missing,
/// provider_unproven, model_not_found) serializes frames free of the
/// canary.
#[test]
fn acp_error_frames_never_carry_credentials() {
    let _guard = env_lock();
    clear_env();
    let canary = format!("sk-C8CANARY-frames-{}", std::process::id());
    unsafe { std::env::set_var("OPENAI_API_KEY", &canary) };
    let router = parity_router();
    let available = advertised_with(&router);
    let mut host = Host::spawn(router, available, "flux-auto");

    let new_resp = host.request(
        "session/new",
        serde_json::json!({"cwd": std::env::temp_dir()}),
    );
    let session_id = new_resp["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // Successful switch (keyed, proven via seam) — ok frame.
    let ok = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session_id, "modelId": "openai:gpt-5.6-terra"}),
    );
    // Unkeyed provider — provider_key_missing.
    let missing = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session_id, "modelId": "xai:grok-9"}),
    );
    assert_eq!(missing["error"]["data"]["kind"], "provider_key_missing");
    // Unproven arm — provider_unproven.
    let unproven = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session_id, "modelId": "nvidia:nv-model-1"}),
    );
    assert_eq!(unproven["error"]["data"]["kind"], "provider_unproven");
    // Malformed ids — model_not_found.
    for bad in ["a:b:c", "openai:", ":grok-9"] {
        let resp = host.request(
            "session/set_model",
            serde_json::json!({"sessionId": session_id, "modelId": bad}),
        );
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .contains("model_not_found"),
            "{bad}: {resp}"
        );
    }

    for frame in [&ok, &missing, &unproven] {
        let text = serde_json::to_string(frame).unwrap();
        assert!(
            !text.contains(&canary),
            "canary leaked into ACP frame: {text}"
        );
    }
    clear_env();
}
