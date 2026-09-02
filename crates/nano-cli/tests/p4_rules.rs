//! P4 §9/§2.5 integration legs for the rules lane (F-P4-1 wiring): the
//! `wayland-nano rules` CLI surface, the `/doctor` rules-file line, and the
//! session-start fail-closed load surfacing over the live host. The
//! approval-gate matrix itself is pinned in-crate (acp_mode gate tests);
//! the engine battery lives in nano-core (execrules + differential).

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{ModelError, ModelRequest, ModelResponse, ToolCall};
use nano_protocol::acp::AvailableModel;
use std::io::Write;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct UnusedDriver;

#[async_trait::async_trait]
impl ModelDriver for UnusedDriver {
    async fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Protocol(
            "p4 rules harness unexpectedly reached the model".into(),
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
            output: "p4 rules harness unexpectedly reached a tool".into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

#[derive(Clone, Default)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

static DEFAULT_ROUTING: nano_cli::auto_routing::RoutingConfig =
    nano_cli::auto_routing::RoutingConfig {
        auto_opt_in: false,
        configured_default: None,
        tools_probe: false,
    };

/// Write a VALID rules.toml through the real amendment writer (owner-only
/// 0600 on unix, the pinned current-user-only DACL on Windows).
fn seed_valid_rules(home: &std::path::Path) {
    let amendment = nano_core::execrules::mint_amendment(
        "git status",
        if cfg!(windows) {
            nano_core::execrules::ShellGrammar::CmdExe
        } else {
            nano_core::execrules::ShellGrammar::PosixSh
        },
        nano_core::execrules::AmendmentKind::Exact,
        None,
        "2026-08-14T00:00:00Z".into(),
    )
    .unwrap();
    nano_core::execrules::append_amendment(home, None, &amendment).unwrap();
}

fn run_rules(home: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("rules")
        .env("NANO_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn rules_subcommand_reports_the_empty_state() {
    let home = tempfile::tempdir().unwrap();
    let out = run_rules(home.path());
    assert!(out.status.success(), "rc: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rules file:"), "{stdout}");
    assert!(stdout.contains("(no rules)"), "{stdout}");
}

#[test]
fn rules_subcommand_prints_the_parsed_table() {
    let home = tempfile::tempdir().unwrap();
    seed_valid_rules(home.path());
    let out = run_rules(home.path());
    assert!(out.status.success(), "rc: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("#0"), "{stdout}");
    assert!(stdout.contains("allow"), "{stdout}");
    assert!(stdout.contains("exact"), "{stdout}");
    assert!(stdout.contains("git status"), "{stdout}");
    assert!(stdout.contains("approval_card"), "{stdout}");
}

#[test]
fn rules_subcommand_fails_closed_on_a_tampered_file() {
    let home = tempfile::tempdir().unwrap();
    seed_valid_rules(home.path());
    // Corrupt the content in place (permissions persist), then an operator
    // ACL/permission widening is covered engine-side.
    std::fs::write(home.path().join("rules.toml"), "garbage = [").unwrap();
    let out = run_rules(home.path());
    assert_eq!(out.status.code(), Some(1), "rc: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid or insecurely configured"),
        "the RuleFileInvalid presentation: {stderr}"
    );
}

#[test]
fn rules_subcommand_rejects_patterns_beyond_evaluation_bounds() {
    let cases = [
        format!(
            "[[rule]]\npattern = [\"{}\"]\ndecision = \"allow\"\n",
            "x".repeat(4 * 1024 + 1)
        ),
        format!(
            "[[rule]]\npattern = [{}]\ndecision = \"allow\"\n",
            (0..65)
                .map(|index| format!("\"token-{index}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ];

    for contents in cases {
        let home = tempfile::tempdir().unwrap();
        seed_valid_rules(home.path());
        std::fs::write(home.path().join("rules.toml"), contents).unwrap();
        let out = run_rules(home.path());
        assert_eq!(out.status.code(), Some(1), "rc: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("invalid or insecurely configured"),
            "the RuleFileInvalid presentation: {stderr}"
        );
    }
}

#[test]
fn doctor_reports_the_rules_file_line() {
    let home = tempfile::tempdir().unwrap();
    seed_valid_rules(home.path());
    let out = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("doctor")
        .env("NANO_HOME", home.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("rules-file"),
        "doctor carries the rules-file line: {stdout}"
    );
    assert!(
        stdout.contains("1 rule(s), owner-only verified"),
        "the seeded file reports healthy: {stdout}"
    );
}

/// (d) over the wire: a tampered rules.toml in a LIVE host's nano_home fails
/// closed at session start — the session still comes up (zero rules) and the
/// typed warning reaches stderr (the proof's oracle).
#[test]
fn session_start_surfaces_rule_file_invalid_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    seed_valid_rules(home.path());
    std::fs::write(home.path().join("rules.toml"), "garbage = [").unwrap();

    // Assert the exact warning that the ACP session-start seam writes to
    // stderr, without weakening the production activation boundary merely
    // to capture a child process's stderr.
    let (_, warning) = nano_cli::shell_rules::load_session_rules(home.path());
    let warning = warning.expect("tampered rules file emits a typed warning");
    assert!(
        warning.contains("invalid or insecurely configured"),
        "the typed stderr warning: {warning}"
    );

    let workspace = home.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let input = format!(
        "{}\n{}\n",
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd": workspace, "mcpServers": []}})
    );
    let output = SharedWriter::default();
    let captured = output.clone();
    let sessions_dir = home.path().join("sessions");
    let memory = acp_mode::MemoryHostConfig {
        dir: home.path().join("memory"),
        write_enabled: false,
        block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
        policy: nano_cli::memory_policy::ResolvedMemoryPolicy::disabled(),
    };
    let available_models = [AvailableModel {
        id: "mock".into(),
        name: "mock".into(),
    }];
    let sandbox_probe = || true;
    let router = nano_cli::provider_router::ProviderRouter::default();
    let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
        .expect("vendored vision catalog parses");
    let hooks = nano_hooks::HookEngine::empty();
    let config = acp_mode::ServeConfig {
        sessions_dir: &sessions_dir,
        default_model: "mock",
        available_models: &available_models,
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
        attachment_home: home.path(),
        hooks: &hooks,
        routing: &DEFAULT_ROUTING,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = runtime.block_on(acp_mode::serve_legacy_debug(
        std::io::Cursor::new(input),
        output,
        &config,
        |_| UnusedDriver,
        |_, _, _, _, _, _| {
            (
                UnusedTools,
                nano_core::permissions::PermissionProfile::workspace_write()
                    .file_system_sandbox_policy(),
            )
        },
    ));
    assert_eq!(result.unwrap(), 0);

    // session/new must SUCCEED (zero rules, never a host failure).
    let bytes = captured.0.lock().unwrap().clone();
    let lines = String::from_utf8(bytes).unwrap();
    let newed: serde_json::Value =
        serde_json::from_str(lines.lines().nth(1).expect("session/new response line")).unwrap();
    assert!(
        newed.get("result").is_some(),
        "session/new succeeds with zero rules: {newed}"
    );
}
