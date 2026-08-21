//! P4 §9/§2.5 integration legs for the rules lane (F-P4-1 wiring): the
//! `wayland-nano rules` CLI surface, the `/doctor` rules-file line, and the
//! session-start fail-closed load surfacing over the live host. The
//! approval-gate matrix itself is pinned in-crate (acp_mode gate tests);
//! the engine battery lives in nano-core (execrules + differential).

use std::io::Read;
use std::process::{Command, Stdio};

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
    let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("acp-host")
        .env("NANO_HOME", home.path())
        // Startup needs SOME resolvable credential (B2 gate); this dummy
        // never leaves the process (no model call is made).
        .env("FLUX_API_KEY", "sk-test-fixture-never-networked")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let workspace = home.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    use std::io::Write;
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}})
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd": workspace, "mcpServers": []}})
    )
    .unwrap();
    stdin.flush().unwrap();
    // session/new must SUCCEED (zero rules, never a host failure).
    let mut lines = String::new();
    let mut line = String::new();
    use std::io::BufRead;
    while lines.lines().count() < 2 {
        line.clear();
        if stdout.read_line(&mut line).unwrap() == 0 {
            break;
        }
        lines.push_str(&line);
    }
    let newed: serde_json::Value =
        serde_json::from_str(lines.lines().nth(1).expect("session/new response line")).unwrap();
    assert!(
        newed.get("result").is_some(),
        "session/new succeeds with zero rules: {newed}"
    );
    drop(stdin);
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(
        stderr.contains("invalid or insecurely configured"),
        "the typed warning reaches stderr: {stderr}"
    );
}
