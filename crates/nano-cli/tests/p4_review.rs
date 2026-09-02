//! P4 §3.4/§9 ACP-surface tests for `_wayland/session/review`: the typed
//! refusals and the advertisement discipline. The happy-path child run is
//! covered engine-side (nano-agent tasks.rs review battery with a scripted
//! driver); the LIVE proof is §14 leg 2 (proof lane, FLUX_TEST_KEY-gated).

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{ModelError, ModelRequest, ModelResponse, ToolCall};
use nano_protocol::acp::AvailableModel;
use std::io::{BufRead, Read, Write};

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
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        while let Some(pos) = self.buf.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.tx
                .send(String::from_utf8_lossy(&line).into_owned())
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct UnusedDriver;

#[async_trait::async_trait]
impl ModelDriver for UnusedDriver {
    async fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Protocol(
            "p4 review harness unexpectedly reached the model".into(),
        ))
    }
}

#[derive(Clone, Debug)]
struct UnusedTools;

#[async_trait::async_trait]
impl ToolExecutor for UnusedTools {
    async fn execute(&self, _call: &ToolCall) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            output: "p4 review harness unexpectedly reached a tool".into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

static DEFAULT_ROUTING: nano_cli::auto_routing::RoutingConfig =
    nano_cli::auto_routing::RoutingConfig {
        auto_opt_in: false,
        configured_default: None,
        tools_probe: false,
    };

struct Host {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    handle: std::thread::JoinHandle<std::io::Result<i32>>,
}

impl Host {
    fn spawn(home: &std::path::Path) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let sessions_dir = home.join("sessions");
        let memory_dir = home.join("memory");
        let attachment_home = home.to_path_buf();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let catalog = [AvailableModel {
                    id: "mock".into(),
                    name: "mock".into(),
                }];
                let sandbox_probe = || true;
                let router = nano_cli::provider_router::ProviderRouter::default();
                let memory = acp_mode::MemoryHostConfig {
                    dir: memory_dir,
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                    policy: nano_cli::memory_policy::ResolvedMemoryPolicy::disabled(),
                };
                let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
                    .expect("vendored vision catalog parses");
                let hooks = nano_hooks::HookEngine::empty();
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir,
                    default_model: "mock",
                    available_models: &catalog,
                    env_mcp_specs: &[],
                    catalog: &[],
                    window_override: None,
                    limit_override: None,
                    sandbox_probe: &sandbox_probe,
                    router: &router,
                    journal_append_failer: None,
                    memory: &memory,
                    reasoning_effort: None,
                    verbosity: None,
                    cron_home: None,
                    search: None,
                    search_meter: None,
                    pricing: None,
                    budget_cap: None,
                    vision_catalog: &vision_catalog,
                    attachment_home: &attachment_home,
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
                    |_| UnusedDriver,
                    |_, _, _, _, _, _| {
                        (
                            UnusedTools,
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
            handle,
        }
    }

    fn request(&mut self, id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.to_host
            .as_ref()
            .unwrap()
            .send(format!(
                "{}\n",
                serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            ))
            .unwrap();
        let line = self.frames.recv().unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn initialize(&mut self) -> serde_json::Value {
        self.request(
            1,
            "initialize",
            serde_json::json!({"protocolVersion":1,"clientCapabilities":{}}),
        )
    }

    fn shutdown(mut self) {
        drop(self.to_host.take());
        assert_eq!(self.handle.join().unwrap().unwrap(), 0);
    }
}

#[test]
fn review_requires_a_session_and_takes_no_params() {
    let home = tempfile::tempdir().unwrap();
    let mut host = Host::spawn(home.path());
    host.initialize();

    // No session ⇒ typed no_session.
    let no_session = host.request(2, "_wayland/session/review", serde_json::json!({}));
    assert_eq!(
        no_session["error"]["data"]["nanoError"]["kind"], "no_session",
        "{no_session}"
    );

    // A session on a NON-git workspace ⇒ typed invalid_params with the
    // bounded non-git reason (§3.3/§8: precondition failures ride
    // InvalidParams, no new table kind).
    let plain = home.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let newed = host.request(
        3,
        "session/new",
        serde_json::json!({"cwd": plain, "mcpServers": []}),
    );
    assert!(newed.get("result").is_some(), "session/new: {newed}");
    let non_git = host.request(4, "_wayland/session/review", serde_json::json!({}));
    assert_eq!(
        non_git["error"]["data"]["nanoError"]["kind"], "invalid_params",
        "{non_git}"
    );
    assert!(
        non_git["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a git workspace"),
        "bounded reason: {non_git}"
    );

    // Non-empty params are a typed rejection, never silently ignored.
    let with_params = host.request(
        5,
        "_wayland/session/review",
        serde_json::json!({"target": "HEAD~1"}),
    );
    assert_eq!(
        with_params["error"]["data"]["nanoError"]["kind"], "invalid_params",
        "{with_params}"
    );

    host.shutdown();
}

/// The honesty rule (§9), now satisfied: the `nanoExtensions` advertisement
/// was pinned OFF until the §14 leg-2 live proof — the P4 adversarial
/// proof's leg 7 (post-merge @ 3f9bf87) ran every review-mode leg GREEN, so
/// the pin flipped. This pin now asserts the advertisement is PRESENT and
/// versioned — a regression that drops it fails here.
#[test]
fn review_advertised_after_live_proof() {
    let home = tempfile::tempdir().unwrap();
    let mut host = Host::spawn(home.path());
    let initialize = host.initialize();
    assert_eq!(
        initialize["result"]["agentCapabilities"]["nanoExtensions"]["_wayland/session/review"],
        serde_json::json!({ "version": 1 }),
        "the review extension is advertised (live-proven): {initialize}"
    );
    host.shutdown();
}
