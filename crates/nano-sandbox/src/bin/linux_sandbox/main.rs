//! `nanok3-linux-sandbox` helper: applies the legacy Landlock + seccomp
//! sandbox in-process, then execs the wrapped command.
//!
//! Provenance: ported from Codex `codex-rs/linux-sandbox/src/linux_run_main.rs`
//! @ 646f7c0a, LEGACY Landlock path only. Transformations:
//! - codex_protocol -> nano_core::permissions;
//! - bubblewrap pipeline (default in the donor), proxy routing, synthetic
//!   mounts, protected-create monitoring, signal forwarding and the
//!   `--apply-seccomp-then-exec` / `--no-proc` / `--allow-network-for-proxy`
//!   / `--proxy-route-spec` flags are NOT ported (bwrap is a 2763-line
//!   pipeline of its own; nano-egress owns proxying). TODO: port the bwrap
//!   pipeline when it lands as a follow-up;
//! - the donor's `ensure_legacy_landlock_mode_supports_policy` is not ported:
//!   it relied on the legacy `SandboxPolicy` semantic-signature machinery that
//!   nano-k3 deliberately never ported (greenfield). Fail-closed coverage is
//!   preserved: policies the legacy backend cannot enforce still error inside
//!   `apply_permission_profile_to_current_thread`;
//! - errors panic (fail-closed: the sandboxed command never runs when
//!   restrictions cannot be applied), matching the donor helper.

#[cfg(target_os = "linux")]
mod linux {
    use clap::Parser;
    use std::ffi::CString;
    use std::path::PathBuf;

    use nano_core::permissions::PermissionProfile;

    use crate::landlock::apply_permission_profile_to_current_thread;

    #[derive(Debug, Parser)]
    /// CLI surface for the Linux sandbox helper (legacy Landlock mode).
    pub struct LandlockCommand {
        /// It is possible that the cwd used in the context of the sandbox policy
        /// is different from the cwd of the process to spawn.
        #[arg(long = "sandbox-policy-cwd")]
        pub sandbox_policy_cwd: PathBuf,

        /// The logical working directory for the command being sandboxed.
        ///
        /// Accepted for argv compatibility with the argv builder; the legacy
        /// Landlock path executes from the inherited cwd.
        #[arg(long = "command-cwd", hide = true)]
        pub command_cwd: Option<PathBuf>,

        /// Canonical runtime permissions for the command.
        #[arg(
            long = "permission-profile",
            hide = true,
            value_parser = parse_permission_profile
        )]
        pub permission_profile: Option<PermissionProfile>,

        /// Accepted for argv compatibility. Legacy Landlock is the only
        /// filesystem pipeline in this build, so the flag is a no-op.
        #[arg(long = "use-legacy-landlock", hide = true, default_value_t = false)]
        pub use_legacy_landlock: bool,

        /// Full command args to run under the Linux sandbox helper.
        #[arg(trailing_var_arg = true)]
        pub command: Vec<String>,
    }

    fn parse_permission_profile(value: &str) -> Result<PermissionProfile, String> {
        serde_json::from_str(value).map_err(|err| format!("invalid permission profile JSON: {err}"))
    }

    /// Entry point for the Linux sandbox helper.
    ///
    /// The sequence is:
    /// 1. Apply in-process restrictions (no_new_privs + seccomp + Landlock).
    /// 2. `execvp` into the final command.
    pub fn run_main() -> ! {
        let LandlockCommand {
            sandbox_policy_cwd,
            command_cwd,
            permission_profile,
            use_legacy_landlock,
            command,
        } = LandlockCommand::parse();
        // Compatibility-only surface: accepted so the argv builder's flags
        // parse, but the legacy Landlock path does not consume them.
        let _ = (command_cwd, use_legacy_landlock);

        if command.is_empty() {
            panic!("No command specified to execute.");
        }
        let permission_profile = permission_profile
            .unwrap_or_else(|| panic!("missing permission profile configuration"));

        if let Err(e) = apply_permission_profile_to_current_thread(
            &permission_profile,
            &sandbox_policy_cwd,
        ) {
            panic!("error applying legacy Linux sandbox restrictions: {e:?}");
        }
        exec_or_panic(command);
    }

    fn exec_or_panic(command: Vec<String>) -> ! {
        #[expect(clippy::expect_used)]
        let c_command =
            CString::new(command[0].as_str()).expect("Failed to convert command to CString");
        #[expect(clippy::expect_used)]
        let c_args: Vec<CString> = command
            .iter()
            .map(|arg| CString::new(arg.as_str()).expect("Failed to convert arg to CString"))
            .collect();

        let mut c_args_ptrs: Vec<*const libc::c_char> =
            c_args.iter().map(|arg| arg.as_ptr()).collect();
        c_args_ptrs.push(std::ptr::null());

        unsafe {
            libc::execvp(c_command.as_ptr(), c_args_ptrs.as_ptr());
        }

        // If execvp returns, there was an error.
        let err = std::io::Error::last_os_error();
        panic!("Failed to execvp {}: {err}", command[0].as_str());
    }
}

#[cfg(target_os = "linux")]
mod landlock;

#[cfg(target_os = "linux")]
fn main() {
    linux::run_main();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    panic!("nanok3-linux-sandbox is Linux-only");
}
