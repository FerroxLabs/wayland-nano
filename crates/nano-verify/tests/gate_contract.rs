use nano_verify::gate::{
    CheckVerdict, FailCategory, FailClosedReason, GateInvocation, GateOutcome, run_gate,
};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MODE_ENV: &str = "NANO_VERIFY_FIXTURE_MODE";
const LEAK_ENV: &str = "NANO_VERIFY_TEST_LEAK";
const ENV_DIAGNOSTIC: &str = "NANO_VERIFY_ENV_DIAGNOSTIC";

fn env_key(name: &OsStr) -> String {
    let name = name.to_string_lossy();
    if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.into_owned()
    }
}

fn fixture_entry() -> bool {
    let Some(mode) = std::env::var_os(MODE_ENV) else {
        return false;
    };
    match mode.to_string_lossy().as_ref() {
        "nonzero" => {
            println!("gate: 1/1");
            std::process::exit(23);
        }
        "red" => {
            println!("FAIL TG-01 value\ngate: 0/1");
            std::process::exit(23);
        }
        "argv" => {
            let args: Vec<_> = std::env::args_os().collect();
            let expected = std::env::var_os("NANO_VERIFY_EXPECTED_ARTIFACT").unwrap();
            if args.last() == Some(&expected) {
                println!("gate: 1/1");
                std::process::exit(0);
            }
            std::process::exit(24);
        }
        "env" => {
            let allowed = baseline_names()
                .into_iter()
                .chain([
                    MODE_ENV,
                    "NANO_VERIFY_EXPECTED_ARTIFACT",
                    "NANO_VERIFY_DECLARED",
                    ENV_DIAGNOSTIC,
                ])
                .map(|name| env_key(OsStr::new(name)))
                .collect::<std::collections::BTreeSet<_>>();
            let unexpected = std::env::vars_os()
                .map(|(key, _)| env_key(&key))
                .filter(|key| !allowed.contains(key))
                .collect::<Vec<_>>();
            if !unexpected.is_empty()
                && let Some(path) = std::env::var_os(ENV_DIAGNOSTIC)
            {
                std::fs::write(path, unexpected.join("\n")).unwrap();
            }
            let declared =
                std::env::var_os("NANO_VERIFY_DECLARED").as_deref() == Some(OsStr::new("override"));
            if unexpected.is_empty() && declared && std::env::var_os(LEAK_ENV).is_none() {
                println!("gate: 1/1");
                std::process::exit(0);
            }
            std::process::exit(25);
        }
        "timeout" => {
            let marker = std::env::var_os("NANO_VERIFY_DESCENDANT_MARKER").unwrap();
            let exe = std::env::current_exe().unwrap();
            let mut child = std::process::Command::new(exe)
                .args(["--exact", "fixture_process", "--nocapture"])
                .env(MODE_ENV, "descendant")
                .env("NANO_VERIFY_DESCENDANT_MARKER", marker)
                .spawn()
                .unwrap();
            std::fs::write(
                std::env::var_os("NANO_VERIFY_PID_FILE").unwrap(),
                child.id().to_string(),
            )
            .unwrap();
            let _ = child.wait();
            std::process::exit(26);
        }
        "descendant" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        "bounded" => {
            use std::io::Write;
            let chunk = [b'x'; 8192];
            let mut stdout = std::io::stdout().lock();
            for _ in 0..=2048 {
                stdout.write_all(&chunk).unwrap();
            }
            writeln!(stdout, "gate: 1/1").unwrap();
            std::process::exit(0);
        }
        _ => std::process::exit(27),
    }
}

#[test]
fn fixture_process() {
    let _ = fixture_entry();
}

fn baseline_names() -> Vec<&'static str> {
    let mut names = vec!["PATH", "HOME", "TMPDIR", "TEMP", "TMP"];
    if cfg!(windows) {
        names.extend(["SYSTEMROOT", "PATHEXT", "USERPROFILE", "COMSPEC"]);
    }
    names
}

fn invocation(mode: &str, timeout: Duration, extra: &[(&str, OsString)]) -> GateInvocation {
    let exe = std::env::current_exe().unwrap();
    let mut env = vec![(MODE_ENV.into(), mode.into())];
    env.extend(extra.iter().map(|(k, v)| (OsString::from(k), v.clone())));
    GateInvocation {
        argv: vec![
            exe.into_os_string(),
            "--exact".into(),
            "fixture_process".into(),
            "--nocapture".into(),
        ],
        cwd: std::env::current_dir().unwrap(),
        env,
        timeout,
        gate_id: "fixture".into(),
    }
}

async fn run(mode: &str, artifact: &Path, extra: &[(&str, OsString)]) -> GateOutcome {
    run_gate(
        &invocation(mode, Duration::from_secs(5), extra),
        artifact,
        &inventory(),
    )
    .await
}

fn inventory() -> Vec<(String, FailCategory)> {
    vec![("TG-01".into(), FailCategory::Value)]
}

#[tokio::test]
async fn run_gate_parses_stdout_despite_nonzero_exit() {
    assert_eq!(
        run("nonzero", Path::new("artifact"), &[]).await,
        GateOutcome::Green {
            verdicts: vec![CheckVerdict {
                id: "TG-01".into(),
                category: FailCategory::Value,
                passed: true,
            }],
        }
    );
    assert_eq!(
        run("red", Path::new("artifact"), &[]).await,
        GateOutcome::Red {
            verdicts: vec![CheckVerdict {
                id: "TG-01".into(),
                category: FailCategory::Value,
                passed: false,
            }],
        }
    );
}

#[tokio::test]
async fn run_gate_spawn_error_fails_closed() {
    let mut inv = invocation("nonzero", Duration::from_secs(1), &[]);
    inv.argv[0] = OsString::from("wayland-nano-definitely-absent-gate-program");
    assert!(
        matches!(run_gate(&inv, Path::new("artifact"), &inventory()).await, GateOutcome::FailClosed(FailClosedReason::SpawnError(message)) if message.len() <= 96)
    );
}

#[tokio::test]
async fn run_gate_artifact_path_is_final_argv() {
    let artifact = PathBuf::from("synthetic artifact ; no shell expansion");
    let extra = [(
        "NANO_VERIFY_EXPECTED_ARTIFACT",
        artifact.clone().into_os_string(),
    )];
    assert!(matches!(
        run("argv", &artifact, &extra).await,
        GateOutcome::Green { .. }
    ));
}

#[tokio::test]
async fn run_gate_env_baseline_allowlist() {
    unsafe {
        std::env::set_var(LEAK_ENV, "synthetic-secret-marker");
    }
    let temp = tempfile::tempdir().unwrap();
    let diagnostic = temp.path().join("unexpected-env-keys.txt");
    let declared_name = if cfg!(windows) {
        "nano_verify_declared"
    } else {
        "NANO_VERIFY_DECLARED"
    };
    let extra = [
        (declared_name, OsString::from("override")),
        (ENV_DIAGNOSTIC, diagnostic.clone().into_os_string()),
    ];
    let outcome = run("env", Path::new("artifact"), &extra).await;
    unsafe {
        std::env::remove_var(LEAK_ENV);
    }
    let unexpected = std::fs::read_to_string(diagnostic).unwrap_or_default();
    assert!(
        matches!(outcome, GateOutcome::Green { .. }),
        "unexpected environment keys (values omitted): {unexpected}; outcome: {outcome:?}"
    );
}

#[tokio::test]
async fn run_gate_timeout_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("descendant.pid");
    let marker = format!("nano-verify-descendant-{}", std::process::id());
    let extra = [
        ("NANO_VERIFY_PID_FILE", pid_file.clone().into_os_string()),
        ("NANO_VERIFY_DESCENDANT_MARKER", OsString::from(marker)),
    ];
    let outcome = run_gate(
        &invocation("timeout", Duration::from_millis(500), &extra),
        Path::new("artifact"),
        &inventory(),
    )
    .await;
    assert_eq!(outcome, GateOutcome::FailClosed(FailClosedReason::Timeout));
    let pid: u32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !process_alive(pid),
        "descendant {pid} survived whole-tree timeout and bounded reap window"
    );

    let bounded = run("bounded", Path::new("artifact"), &[]).await;
    assert_eq!(
        bounded,
        GateOutcome::FailClosed(FailClosedReason::NoGateOutput)
    );
}

#[tokio::test]
async fn run_gate_empty_inventory_fails_closed() {
    assert_eq!(
        run_gate(
            &invocation("nonzero", Duration::from_secs(5), &[]),
            Path::new("artifact"),
            &[],
        )
        .await,
        GateOutcome::FailClosed(FailClosedReason::InconsistentSummary {
            passed: 1,
            total: 1,
        })
    );
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle == 0 {
        return false;
    }
    let mut code = 0;
    let ok = unsafe { GetExitCodeProcess(handle, &mut code) } != 0;
    unsafe {
        CloseHandle(handle);
    }
    ok && code == 259
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 }
}
