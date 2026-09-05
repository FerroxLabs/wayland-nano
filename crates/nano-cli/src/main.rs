//! wayland-nano — Wayland Nano (Track B) binary: doctor, protocol host.

mod doctor;
mod host_mode;
mod memory_migrate;

use nano_cli::acp_mode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exit_code = match args.get(1).map(String::as_str) {
        Some("doctor") => {
            let nano_home = nano_home();
            let mut out = std::io::stdout();
            doctor::run(&nano_home, &mut out).unwrap_or(2)
        }
        Some("acp-host") => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let home = nano_home();
            let nonpersistent =
                args.get(2).map(String::as_str) == Some("--nonpersistent") && args.len() == 3;
            if args.len() > 2 && !nonpersistent {
                eprintln!("usage: wayland-nano acp-host [--nonpersistent]");
                std::process::exit(2);
            }
            let result = if nonpersistent {
                runtime.block_on(acp_mode::run_nonpersistent(&home))
            } else {
                runtime.block_on(acp_mode::run(&home))
            };
            match result {
                Ok(code) => code,
                Err(err) => {
                    eprintln!("wayland-nano: acp io error: {err}");
                    2
                }
            }
        }
        Some("protocol-host") => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let home = nano_home();
            let workspace = std::env::current_dir().expect("cwd");
            let activation_request = args
                .windows(2)
                .find(|pair| pair[0] == "--activation-request")
                .map(|pair| std::path::PathBuf::from(&pair[1]));
            match runtime.block_on(host_mode::run(
                &home,
                &workspace,
                activation_request.as_deref(),
            )) {
                Ok(host_mode::HostExit::StdinClosed) => 0,
                Ok(host_mode::HostExit::ShutdownCommand) => 0,
                Ok(host_mode::HostExit::Fatal(reason)) => {
                    eprintln!("wayland-nano: fatal: {reason}");
                    2
                }
                Err(err) => {
                    eprintln!("wayland-nano: host loop io error: {err}");
                    2
                }
            }
        }
        // P3 §6.2/§8 (F-P3-1): OAuth for remote (HTTP) MCP servers.
        Some("auth") => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let home = nano_home();
            runtime.block_on(nano_cli::auth_cmds::run(&home, &args[2..]))
        }
        // C11: headless one-shot execution (JSONL v1 on stdout; the pinned
        // 0/1/2/3/6 exit-code matrix).
        Some("exec") => {
            let home = nano_home();
            let workspace = std::env::current_dir().expect("cwd");
            match parse_exec_args(&args[2..]) {
                Err(code) => code,
                Ok(parsed) => {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("tokio runtime");
                    match parsed.local_activation.as_ref() {
                        Some(local) => {
                            runtime.block_on(nano_cli::exec_run::run_with_local_activation(
                                &home,
                                &workspace,
                                &parsed.exec,
                                Some(local),
                            ))
                        }
                        None => runtime.block_on(nano_cli::exec_run::run(
                            &home,
                            &workspace,
                            &parsed.exec,
                        )),
                    }
                }
            }
        }
        Some("activation") => {
            let home = nano_home();
            let mut out = std::io::stdout();
            nano_cli::activation::run_activation_command(&home, &args[2..], &mut out)
        }
        Some("admin") => {
            let home = nano_home();
            let mut out = std::io::stdout();
            nano_cli::activation::run_admin_command(&home, &args[2..], &mut out)
        }
        Some("memory") => {
            let home = nano_home();
            let mut out = std::io::stdout();
            let mut err = std::io::stderr();
            memory_migrate::run(&home, &args[2..], &mut out, &mut err)
        }
        // C11: session fork — clone a journal prefix under the SessionGuard.
        Some("session") if args.get(2).map(String::as_str) == Some("fork") => {
            let home = nano_home();
            let sessions_dir = home.join("sessions");
            let mut at_turn = None;
            let mut positional = Vec::new();
            let mut index = 3;
            while let Some(arg) = args.get(index) {
                match arg.as_str() {
                    "--at-turn" => {
                        at_turn = args.get(index + 1).cloned();
                        index += 2;
                    }
                    other => {
                        positional.push(other.to_string());
                        index += 1;
                    }
                }
            }
            let Some(session_id) = positional.first() else {
                eprintln!("usage: wayland-nano session fork <session_id> [--at-turn <turn_id>]");
                std::process::exit(2);
            };
            let mut out = std::io::stdout();
            nano_cli::session_cmds::session_fork(&sessions_dir, session_id, at_turn, &mut out)
        }
        // C11: goal lifecycle CLI mirrors (thin journal readers/writers).
        Some("goal") => {
            let home = nano_home();
            let sessions_dir = home.join("sessions");
            let mut out = std::io::stdout();
            goal_command(&sessions_dir, &args[2..], &mut out)
        }
        Some("sessions") => {
            let sessions_dir = nano_home().join("sessions");
            let mut out = std::io::stdout();
            nano_cli::session_browser::print_sessions(&sessions_dir, &mut out)
        }
        // P4 §9: the shell-rule table (read-only; codex's `execpolicy
        // check` debugging surface, minimal form).
        Some("rules") => {
            let home = nano_home();
            let mut out = std::io::stdout();
            nano_cli::rules_cmds::run(&home, &mut out)
        }
        Some("plugin") => {
            let home = nano_home();
            let mut out = std::io::stdout();
            nano_cli::plugin_cmds::run(&home, &args[2..], &mut out)
        }
        // WP3: gated-climb verification + red-green receipts (JSONL v1 on
        // stdout; the verify exit matrix 0/1/2/3/6, §2 of SPEC-WP3).
        Some("verify") => {
            let home = nano_home();
            let workspace = std::env::current_dir().expect("cwd");
            match nano_cli::verify_cmd::parse_args(&args[2..]) {
                Err(code) => code,
                Ok(params) => {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("tokio runtime");
                    runtime.block_on(nano_cli::verify_cmd::run(&home, &workspace, &params))
                }
            }
        }
        Some("--version") | Some("-V") => {
            println!("wayland-nano {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!(
                "usage: wayland-nano doctor | protocol-host | acp-host | auth login|status|logout <server> | exec | session fork | sessions | rules | plugin | verify | goal | memory migrate | --version"
            );
            2
        }
    };
    std::process::exit(exit_code);
}

/// Parses `exec` arguments. Usage errors are exit 2 (the caller prints).
struct ParsedExec {
    exec: nano_cli::exec_mode::ExecParams,
    local_activation: Option<nano_cli::exec_mode::LocalActivationParams>,
}

fn parse_exec_args(args: &[String]) -> Result<ParsedExec, i32> {
    let mut mode = nano_protocol::permission_mode::PermissionMode::default();
    let mut resume = None;
    let mut output_last_message = None;
    let mut goal: Option<nano_cli::exec_mode::ExecGoal> = None;
    // P5 §1: the explicit model pin and the Auto opt-in.
    let mut model = None;
    let mut auto = false;
    let mut activation_request = None;
    let mut activation_keyref = None;
    let mut activation_issuer = None;
    let mut activation_key_id = None;
    let mut activation_project = None;
    let mut activation_resume_fingerprint = None;
    let mut positional = Vec::new();
    let mut index = 0;
    let take_value = |args: &[String], index: &mut usize, flag: &str| -> Result<String, i32> {
        *index += 1;
        match args.get(*index) {
            Some(value) => Ok(value.clone()),
            None => {
                eprintln!("wayland-nano: {flag} requires a value");
                eprintln!(
                    "usage: wayland-nano exec [--mode read_only|default|full_auto] \
                     [--model <id>] [--auto] \
                     [--resume <id> | --resume-last] [--output-last-message <file>] \
                     [--goal <objective> [--token-budget N] [--turn-budget N] \
                     [--wall-clock-budget MS]] [prompt]"
                );
                Err(2)
            }
        }
    };
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--mode" => {
                let value = take_value(args, &mut index, "--mode")?;
                mode = nano_cli::session_cmds::parse_mode(&value).map_err(|err| {
                    eprintln!("wayland-nano: {err}");
                    2
                })?;
            }
            // P5 §1: an explicit CLI model pin (bare Flux id — exec's wire is
            // Flux-only today; namespaced pins are the acp-host's C8 surface).
            "--model" => {
                let value = take_value(args, &mut index, "--model")?;
                model = Some(value);
            }
            // P5 §1: the explicit Auto opt-in (the resolved reference must be
            // `flux-auto`; a pin above wins over the opt-in — precedence).
            "--auto" => {
                auto = true;
            }
            "--activation-request" => {
                activation_request = Some(std::path::PathBuf::from(take_value(
                    args,
                    &mut index,
                    "--activation-request",
                )?));
            }
            "--activation-keyref" => {
                activation_keyref = Some(std::path::PathBuf::from(take_value(
                    args,
                    &mut index,
                    "--activation-keyref",
                )?))
            }
            "--activation-issuer" => {
                activation_issuer = Some(take_value(args, &mut index, "--activation-issuer")?)
            }
            "--activation-key-id" => {
                activation_key_id = Some(take_value(args, &mut index, "--activation-key-id")?)
            }
            "--activation-project" => {
                activation_project = Some(take_value(args, &mut index, "--activation-project")?)
            }
            "--activation-resume-fingerprint" => {
                activation_resume_fingerprint = Some(take_value(
                    args,
                    &mut index,
                    "--activation-resume-fingerprint",
                )?)
            }
            "--resume" => {
                let value = take_value(args, &mut index, "--resume")?;
                resume = Some(nano_cli::exec_mode::ResumeTarget::Id(value));
            }
            "--resume-last" => {
                resume = Some(nano_cli::exec_mode::ResumeTarget::Last);
            }
            "--output-last-message" => {
                let value = take_value(args, &mut index, "--output-last-message")?;
                output_last_message = Some(std::path::PathBuf::from(value));
            }
            "--goal" => {
                let value = take_value(args, &mut index, "--goal")?;
                goal = Some(nano_cli::exec_mode::ExecGoal {
                    objective: value,
                    budgets: nano_session::GoalBudgets::default(),
                });
            }
            "--token-budget" | "--turn-budget" | "--wall-clock-budget" => {
                let flag = arg.clone();
                let value = take_value(args, &mut index, &flag)?;
                let parsed: u64 = value.parse().map_err(|_| {
                    eprintln!("wayland-nano: {flag} must be a non-negative integer");
                    2
                })?;
                let Some(goal) = goal.as_mut() else {
                    eprintln!("wayland-nano: {flag} requires --goal");
                    return Err(2);
                };
                match flag.as_str() {
                    "--token-budget" => goal.budgets.token_budget = Some(parsed),
                    "--turn-budget" => goal.budgets.turn_budget = Some(parsed),
                    _ => goal.budgets.wall_clock_budget_ms = Some(parsed),
                }
            }
            other if other.starts_with("--") => {
                eprintln!("wayland-nano: unknown exec flag: {other}");
                return Err(2);
            }
            other => positional.push(other.to_string()),
        }
        index += 1;
    }
    let prompt = positional.join(" ");
    if prompt.is_empty() && goal.is_none() {
        eprintln!("wayland-nano: exec requires a prompt (or --goal)");
        return Err(2);
    }
    if activation_request.is_some() && activation_keyref.is_some() {
        eprintln!("wayland-nano: --activation-request conflicts with local activation flags");
        return Err(2);
    }
    let local_count = [
        activation_keyref.is_some(),
        activation_issuer.is_some(),
        activation_key_id.is_some(),
        activation_project.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if local_count != 0 && local_count != 4 {
        eprintln!(
            "wayland-nano: local activation requires --activation-keyref, --activation-issuer, --activation-key-id, and --activation-project"
        );
        return Err(2);
    }
    if activation_resume_fingerprint.is_some() && activation_keyref.is_none() {
        eprintln!("wayland-nano: --activation-resume-fingerprint requires local activation flags");
        return Err(2);
    }
    let local_session = match &resume {
        Some(nano_cli::exec_mode::ResumeTarget::Id(id)) => Some(id.clone()),
        Some(nano_cli::exec_mode::ResumeTarget::Last) if activation_keyref.is_some() => {
            eprintln!(
                "wayland-nano: local activation requires explicit --resume <id>, not --resume-last"
            );
            return Err(2);
        }
        _ => None,
    };
    if activation_keyref.is_some()
        && (local_session.is_some() != activation_resume_fingerprint.is_some())
    {
        eprintln!(
            "wayland-nano: local resume requires --resume <id> and --activation-resume-fingerprint together"
        );
        return Err(2);
    }
    let local_activation =
        activation_keyref.map(|key_reference| nano_cli::exec_mode::LocalActivationParams {
            key_reference,
            issuer_id: activation_issuer.unwrap(),
            key_id: activation_key_id.unwrap(),
            project_id: activation_project.unwrap(),
            session_id: local_session,
            resume_fingerprint: activation_resume_fingerprint,
        });
    Ok(ParsedExec {
        exec: nano_cli::exec_mode::ExecParams {
            prompt,
            mode,
            resume,
            output_last_message,
            goal,
            model,
            auto,
            activation_request,
        },
        local_activation,
    })
}

/// `wayland-nano goal set|status|pause|resume|cancel` dispatch.
fn goal_command(
    sessions_dir: &std::path::Path,
    args: &[String],
    out: &mut dyn std::io::Write,
) -> i32 {
    let usage = || {
        eprintln!(
            "usage: wayland-nano goal set <session_id> <objective> [--token-budget N] \
             [--turn-budget N] [--wall-clock-budget MS] | goal status|pause|resume|cancel \
             <session_id>"
        );
        2
    };
    match args.first().map(String::as_str) {
        Some("status") => {
            let Some(session_id) = args.get(1) else {
                return usage();
            };
            nano_cli::session_cmds::goal_status(sessions_dir, session_id, out)
        }
        Some("pause" | "resume" | "cancel") => {
            let (Some(action), Some(session_id)) = (args.first(), args.get(1)) else {
                return usage();
            };
            nano_cli::session_cmds::goal_transition(sessions_dir, session_id, action, out)
        }
        Some("set") => {
            let Some(session_id) = args.get(1) else {
                return usage();
            };
            let mut budgets = nano_session::GoalBudgets::default();
            let mut objective_parts = Vec::new();
            let mut index = 2;
            while let Some(arg) = args.get(index) {
                match arg.as_str() {
                    "--token-budget" | "--turn-budget" | "--wall-clock-budget" => {
                        let Some(value) = args.get(index + 1) else {
                            return usage();
                        };
                        let Ok(parsed) = value.parse::<u64>() else {
                            return usage();
                        };
                        match arg.as_str() {
                            "--token-budget" => budgets.token_budget = Some(parsed),
                            "--turn-budget" => budgets.turn_budget = Some(parsed),
                            _ => budgets.wall_clock_budget_ms = Some(parsed),
                        }
                        index += 2;
                    }
                    other => {
                        objective_parts.push(other.to_string());
                        index += 1;
                    }
                }
            }
            let objective = objective_parts.join(" ");
            nano_cli::session_cmds::goal_set(sessions_dir, session_id, &objective, &budgets, out)
        }
        _ => usage(),
    }
}

fn nano_home() -> std::path::PathBuf {
    std::env::var_os("NANO_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs_next::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".nano")
        })
}
