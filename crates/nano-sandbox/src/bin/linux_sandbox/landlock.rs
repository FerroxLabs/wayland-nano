//! In-process Linux sandbox primitives: `no_new_privs`, Landlock filesystem
//! rules, and the network seccomp filter (legacy enforcement path).
//!
//! Provenance: ported from Codex `codex-rs/linux-sandbox/src/landlock.rs`
//! @ 646f7c0a. Transformations:
//! - codex_protocol errors -> anyhow;
//! - codex_protocol::models/permissions -> nano_core::permissions (+ the
//!   policy_engine behavioral layer);
//! - codex_utils_absolute_path -> nano_core::abs;
//! - managed-network-proxy surface DROPPED (nano-egress owns egress):
//!   `allow_network_for_proxy` / `proxy_routed_network` params and the
//!   `ProxyRouted` seccomp mode are not ported; restricted network always
//!   installs the AF_UNIX-only filter;
//! - this legacy Landlock path is the ONLY filesystem backend in this pass;
//!   bubblewrap (`codex-rs/linux-sandbox/src/bwrap.rs`, 2763 lines) is
//!   intentionally out of scope (TODO: port when the bwrap pipeline lands);
//! - `set_no_new_privs(true)` -> `no_new_privs(true)` (landlock 0.4.7
//!   deprecated the former; same semantics).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::NetworkSandboxPolicy;
use nano_core::permissions::PermissionProfile;

use landlock::ABI;
use landlock::Access;
use landlock::AccessFs;
use landlock::CompatLevel;
use landlock::Compatible;
use landlock::Ruleset;
use landlock::RulesetAttr;
use landlock::RulesetCreatedAttr;
use seccompiler::BpfProgram;
use seccompiler::SeccompAction;
use seccompiler::SeccompCmpArgLen;
use seccompiler::SeccompCmpOp;
use seccompiler::SeccompCondition;
use seccompiler::SeccompFilter;
use seccompiler::SeccompRule;
use seccompiler::TargetArch;
use seccompiler::apply_filter;

/// Apply sandbox policies inside this thread so only the child inherits
/// them, not the entire CLI process.
///
/// This function is responsible for:
/// - enabling `PR_SET_NO_NEW_PRIVS` when restrictions apply,
/// - installing the network seccomp filter when network access is disabled, and
/// - installing the legacy Landlock filesystem ruleset.
pub(crate) fn apply_permission_profile_to_current_thread(
    permission_profile: &PermissionProfile,
    cwd: &Path,
) -> Result<()> {
    let (file_system_sandbox_policy, network_sandbox_policy) =
        permission_profile.to_runtime_permissions();
    let install_network_seccomp = should_install_network_seccomp(network_sandbox_policy);

    // `PR_SET_NO_NEW_PRIVS` is required for seccomp, but it also prevents
    // setuid privilege elevation. We only enable it when restrictions apply.
    if install_network_seccomp || !file_system_sandbox_policy.has_full_disk_write_access() {
        set_no_new_privs()?;
    }

    if install_network_seccomp {
        install_network_seccomp_filter_on_current_thread()?;
    }

    if !file_system_sandbox_policy.has_full_disk_write_access() {
        if !file_system_sandbox_policy.has_full_disk_read_access() {
            anyhow::bail!(
                "Restricted read-only access is not supported by the legacy Linux Landlock filesystem backend."
            );
        }

        let writable_roots = file_system_sandbox_policy
            .get_writable_roots_with_cwd(cwd)
            .into_iter()
            .map(|writable_root| writable_root.root)
            .collect();
        install_filesystem_landlock_rules_on_current_thread(writable_roots)?;
    }

    Ok(())
}

fn should_install_network_seccomp(network_sandbox_policy: NetworkSandboxPolicy) -> bool {
    // Without a managed proxy surface, any restricted network policy installs
    // the fail-closed seccomp filter.
    !network_sandbox_policy.is_enabled()
}

/// Enable `PR_SET_NO_NEW_PRIVS` so seccomp can be applied safely.
fn set_no_new_privs() -> Result<()> {
    let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Installs Landlock file-system rules on the current thread allowing read
/// access to the entire file-system while restricting write access to
/// `/dev/null` and the provided list of `writable_roots`.
///
/// # Errors
/// Returns an error when the ruleset fails to apply.
fn install_filesystem_landlock_rules_on_current_thread(
    writable_roots: Vec<AbsolutePathBuf>,
) -> Result<()> {
    let abi = ABI::V5;
    let access_rw = AccessFs::from_all(abi);
    let access_ro = AccessFs::from_read(abi);

    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(access_rw)?
        .create()?
        .add_rules(landlock::path_beneath_rules(&["/"], access_ro))?
        .add_rules(landlock::path_beneath_rules(&["/dev/null"], access_rw))?
        .no_new_privs(true);

    if !writable_roots.is_empty() {
        ruleset = ruleset.add_rules(landlock::path_beneath_rules(&writable_roots, access_rw))?;
    }

    let status = ruleset.restrict_self()?;

    if status.ruleset == landlock::RulesetStatus::NotEnforced {
        anyhow::bail!("landlock ruleset was not enforced");
    }

    Ok(())
}

/// Installs a seccomp filter for Linux network sandboxing.
///
/// The filter is applied to the current thread so only the sandboxed child
/// inherits it. Restricted mode allows `AF_UNIX` sockets only.
fn install_network_seccomp_filter_on_current_thread() -> std::result::Result<(), anyhow::Error> {
    fn deny_syscall(rules: &mut BTreeMap<i64, Vec<SeccompRule>>, nr: i64) {
        rules.insert(nr, vec![]); // empty rule vec = unconditional match
    }

    // Build rule map.
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();

    deny_syscall(&mut rules, libc::SYS_ptrace);
    deny_syscall(&mut rules, libc::SYS_process_vm_readv);
    deny_syscall(&mut rules, libc::SYS_process_vm_writev);
    deny_syscall(&mut rules, libc::SYS_io_uring_setup);
    deny_syscall(&mut rules, libc::SYS_io_uring_enter);
    deny_syscall(&mut rules, libc::SYS_io_uring_register);

    deny_syscall(&mut rules, libc::SYS_connect);
    deny_syscall(&mut rules, libc::SYS_accept);
    deny_syscall(&mut rules, libc::SYS_accept4);
    deny_syscall(&mut rules, libc::SYS_bind);
    deny_syscall(&mut rules, libc::SYS_listen);
    deny_syscall(&mut rules, libc::SYS_getpeername);
    deny_syscall(&mut rules, libc::SYS_getsockname);
    deny_syscall(&mut rules, libc::SYS_shutdown);
    deny_syscall(&mut rules, libc::SYS_sendto);
    deny_syscall(&mut rules, libc::SYS_sendmmsg);
    // NOTE: allowing recvfrom allows some tools like: `cargo clippy`
    // to run with their socketpair + child processes for sub-proc
    // management.
    // deny_syscall(&mut rules, libc::SYS_recvfrom);
    deny_syscall(&mut rules, libc::SYS_recvmmsg);
    deny_syscall(&mut rules, libc::SYS_getsockopt);
    deny_syscall(&mut rules, libc::SYS_setsockopt);

    // For `socket` we allow AF_UNIX (arg0 == AF_UNIX) and deny
    // everything else.
    let unix_only_rule = SeccompRule::new(vec![SeccompCondition::new(
        0, // first argument (domain)
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::Ne,
        libc::AF_UNIX as u64,
    )?])?;

    rules.insert(libc::SYS_socket, vec![unix_only_rule.clone()]);
    rules.insert(libc::SYS_socketpair, vec![unix_only_rule]);

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,                     // default – allow
        SeccompAction::Errno(libc::EPERM as u32), // when rule matches – return EPERM
        if cfg!(target_arch = "x86_64") {
            TargetArch::x86_64
        } else if cfg!(target_arch = "aarch64") {
            TargetArch::aarch64
        } else {
            unimplemented!("unsupported architecture for seccomp filter");
        },
    )?;

    let prog: BpfProgram = filter.try_into()?;

    apply_filter(&prog)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::should_install_network_seccomp;
    use nano_core::permissions::NetworkSandboxPolicy;
    use pretty_assertions::assert_eq;

    #[test]
    fn full_network_policy_skips_seccomp() {
        assert_eq!(
            should_install_network_seccomp(NetworkSandboxPolicy::Enabled),
            false
        );
    }

    #[test]
    fn restricted_network_policy_always_installs_seccomp() {
        assert!(should_install_network_seccomp(
            NetworkSandboxPolicy::Restricted,
        ));
    }
}
