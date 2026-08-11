//! `wayland-nano doctor` — self-diagnostics for the acceptance flow.
//!
//! Reports environment truth, sandbox state, egress policy health, journal
//! integrity, and process hygiene. Exit 0 when all *required* checks pass;
//! unprovisioned sandbox is a WARN (not a fail) until the elevated setup runs.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

pub struct Check {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
}

pub fn run(nano_home: &std::path::Path, out: &mut dyn std::io::Write) -> std::io::Result<i32> {
    let mut checks = Vec::new();

    checks.push(Check {
        name: "os",
        status: CheckStatus::Pass,
        detail: format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
    });

    #[cfg(windows)]
    {
        checks.push(shell_check("cmd", "cmd.exe", &["/c", "echo wayland-nano"]));
        checks.push(shell_check(
            "powershell",
            "powershell.exe",
            &["-NoProfile", "-Command", "echo wayland-nano"],
        ));
    }
    #[cfg(unix)]
    checks.push(shell_check("sh", "sh", &["-c", "echo wayland-nano"]));

    #[cfg(windows)]
    let setup_complete = nano_sandbox::identity::sandbox_setup_is_complete(nano_home);
    #[cfg(unix)]
    let setup_complete = nano_sandbox::get_platform_sandbox(false).is_some();
    #[cfg(windows)]
    let (setup_pass_detail, setup_warn_detail) = (
        "setup marker + users present (v-matched)",
        "not provisioned — elevated setup required for elevated/elevated-network flows",
    );
    #[cfg(unix)]
    let (setup_pass_detail, setup_warn_detail) = (
        "platform sandbox available (seatbelt/seccomp; no provisioning needed)",
        "no platform sandbox available — shell tool fails closed",
    );
    checks.push(Check {
        name: "sandbox-provisioning",
        status: if setup_complete {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        detail: if setup_complete {
            setup_pass_detail.into()
        } else {
            setup_warn_detail.into()
        },
    });

    let egress = nano_egress::client::EgressClient::flux();
    let deny_ok = egress
        .request(reqwest::Method::GET, "https://example.invalid/")
        .is_err();
    checks.push(Check {
        name: "egress-policy",
        status: if deny_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if deny_ok {
            "non-allowlisted hosts denied at construction".into()
        } else {
            "POLICY FAILURE: non-allowlisted host was allowed".into()
        },
    });

    let journal_probe = probe_journal(nano_home);
    checks.push(Check {
        name: "journal",
        status: journal_probe.0,
        detail: journal_probe.1,
    });

    let sensitive_ok = nano_tools::fs::is_sensitive_path(std::path::Path::new(".env"))
        && !nano_tools::fs::is_sensitive_path(std::path::Path::new("notes.txt"));
    checks.push(Check {
        name: "sensitive-file-policy",
        status: if sensitive_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: ".env denied; notes.txt allowed".into(),
    });

    let strays = stray_nano_processes();
    checks.push(Check {
        name: "process-hygiene",
        status: if strays.is_empty() {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        detail: if strays.is_empty() {
            "no stray wayland-nano-* helper processes".into()
        } else {
            format!("strays: {}", strays.join(","))
        },
    });

    let flux_key = nano_cli::flux_key::flux_api_key().is_some();
    checks.push(Check {
        name: "flux-credential",
        status: if flux_key {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        detail: if flux_key {
            "Flux credential resolvable (env or FLUX_API_KEY_FILE)".into()
        } else {
            "no FLUX_API_KEY / FLUX_API_KEY_FILE resolvable".into()
        },
    });

    let mut failures = 0;
    let mut warnings = 0;
    writeln!(out, "wayland-nano doctor — {}", env!("CARGO_PKG_VERSION"))?;
    for check in &checks {
        let mark = match check.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => {
                warnings += 1;
                "WARN"
            }
            CheckStatus::Fail => {
                failures += 1;
                "FAIL"
            }
        };
        writeln!(out, "  {mark:<5} {:<24} {}", check.name, check.detail)?;
    }
    writeln!(out, "summary: {failures} fail, {warnings} warn")?;
    Ok(if failures > 0 { 1 } else { 0 })
}

fn shell_check(name: &'static str, exe: &str, args: &[&str]) -> Check {
    let found = std::process::Command::new(exe)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    Check {
        name: if name == "cmd" {
            "shell-cmd"
        } else {
            "shell-powershell"
        },
        status: if found {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        detail: if found {
            format!("{exe} executes natively")
        } else {
            format!("{exe} not available")
        },
    }
}

fn probe_journal(nano_home: &std::path::Path) -> (CheckStatus, String) {
    let path = nano_home.join(".doctor-probe.jsonl");
    let result = (|| {
        let mut writer = nano_session::writer::JournalWriter::open(&path)?;
        writer.append(&nano_session::op::OpEnvelope::new(
            "doctor-1",
            "now",
            nano_session::op::Op::SessionBegin {
                session_id: "doctor".into(),
                cwd: ".".into(),
            },
        ))?;
        let report = nano_session::reader::read_journal(&path)?;
        std::fs::remove_file(&path).ok();
        Ok::<usize, std::io::Error>(report.envelopes.len())
    })();
    match result {
        Ok(1) => (CheckStatus::Pass, "append + read-back verified".into()),
        Ok(n) => (
            CheckStatus::Fail,
            format!("replay returned {n} envelopes (expected 1)"),
        ),
        Err(err) => (CheckStatus::Fail, format!("journal probe failed: {err}")),
    }
}

fn stray_nano_processes() -> Vec<String> {
    // Cheap hygiene probe: count OTHER wayland-nano-* processes via WMIC-free
    // approach (tasklist output parse; avoids extra deps).
    let output = std::process::Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let self_pid = std::process::id().to_string();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.to_lowercase().contains("wayland-nano-"))
        .filter(|line| !line.contains(&self_pid))
        .map(|line| line.to_string())
        .collect()
}
