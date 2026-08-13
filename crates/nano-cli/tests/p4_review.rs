//! P4 §3.4/§9 ACP-surface tests for `_wayland/session/review`: the typed
//! refusals and the advertisement discipline. The happy-path child run is
//! covered engine-side (nano-agent tasks.rs review battery with a scripted
//! driver); the LIVE proof is §14 leg 2 (proof lane, FLUX_TEST_KEY-gated).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Host {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Host {
    fn spawn(home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
            .arg("acp-host")
            .env("NANO_HOME", home)
            // Startup needs SOME resolvable credential (B2 gate); this dummy
            // never leaves the process (these tests make NO model call —
            // every leg below ends before or at review spawn gating).
            .env("FLUX_API_KEY", "sk-test-fixture-never-networked")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Host {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }

    fn request(&mut self, id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
        writeln!(
            self.stdin,
            "{}",
            serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
        .unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
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
        drop(self.stdin);
        // The process exits on stdin close; a stuck exit fails loudly here
        // via the assert (never silently leaked).
        assert!(self.child.wait().unwrap().success());
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

/// The honesty rule (§9): the `nanoExtensions` advertisement flips ONLY
/// with the §14 leg-2 live proof. Until the integrator lands that proof
/// and flips the advertisement, this pin asserts the method is NOT
/// advertised — the flip intentionally touches this test.
#[test]
fn review_advertisement_waits_for_live_proof() {
    let home = tempfile::tempdir().unwrap();
    let mut host = Host::spawn(home.path());
    let initialize = host.initialize();
    assert!(
        initialize["result"]["agentCapabilities"]["nanoExtensions"]
            .get("_wayland/session/review")
            .is_none(),
        "advertised before the live proof — honesty rule violation: {initialize}"
    );
    host.shutdown();
}
