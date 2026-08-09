//! nanok3-spawn-profile — measures where spawn prep time goes.
//!
//! The documented ~30s/spawn cost (b-env-02 repro) needs attribution before
//! optimization: restricted-token creation, cap SID loading, session ACL
//! rules, env policy, and the actual CreateProcessAsUserW spawn.

use std::time::Instant;

fn phase<T>(name: &str, started: Instant, f: impl FnOnce() -> T) -> (T, Instant) {
    let out = f();
    let elapsed = started.elapsed();
    println!("{name:<28} {:>8.1} ms", elapsed.as_secs_f64() * 1000.0);
    (out, started)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let tmp = std::env::temp_dir().join("nanok3-prof-fixed");
    let workspace = tmp.join("workspace");
    let nano_home = tmp.join("nano-home");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&nano_home)?;
    std::fs::write(workspace.join("file.txt"), "data")?;

    let overall = Instant::now();
    let profile = nano_core::permissions::PermissionProfile::workspace_write();
    let roots = [nano_core::abs::AbsolutePathBuf::from_absolute_path(
        &workspace,
    )?];

    println!(
        "=== spawn profile: workspace-write on {} ===",
        workspace.display()
    );

    let (_, overall) = phase("resolve permissions", overall, || {
        nano_sandbox::resolved_permissions::ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(&profile, &roots)
            .expect("resolve")
    });

    let permissions = nano_sandbox::resolved_permissions::ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(&profile, &roots)?;

    let (_, overall) = phase("capability roots", overall, || {
        nano_sandbox::spawn_prep::legacy_session_capability_roots(
            &permissions,
            &workspace,
            &std::collections::HashMap::new(),
            &nano_home,
        )
    });

    let roots_caps = nano_sandbox::spawn_prep::legacy_session_capability_roots(
        &permissions,
        &workspace,
        &std::collections::HashMap::new(),
        &nano_home,
    );

    let (_, overall) = phase("session security (token+SIDs)", overall, || {
        nano_sandbox::spawn_prep::prepare_legacy_session_security(
            true, &nano_home, &workspace, roots_caps,
        )
        .expect("session security")
    });

    let security = nano_sandbox::spawn_prep::prepare_legacy_session_security(
        true,
        &nano_home,
        &workspace,
        nano_sandbox::spawn_prep::legacy_session_capability_roots(
            &permissions,
            &workspace,
            &std::collections::HashMap::new(),
            &nano_home,
        ),
    )?;

    let (acl_paths, overall) = phase("compute_allow_paths", overall, || {
        nano_sandbox::allow::compute_allow_paths_for_permissions(
            &permissions,
            &workspace,
            &std::collections::HashMap::new(),
        )
    });
    println!(
        "  allow paths: {}, deny paths: {}",
        acl_paths.allow.len(),
        acl_paths.deny.len()
    );
    for p in &acl_paths.allow {
        println!("    allow: {}", p.display());
    }

    let sid = security.write_root_sids[0].sid.as_ptr();
    let (_, overall) = phase("add_allow_ace xN", overall, || {
        for p in &acl_paths.allow {
            unsafe {
                let _ = nano_sandbox::acl::add_allow_ace(p, sid);
            }
        }
    });

    let (_, overall) = phase("allow_null_device", overall, || unsafe {
        nano_sandbox::acl::allow_null_device(sid);
    });

    let (_, overall) = phase("ensure_allow_write_aces xN", overall, || {
        for p in &acl_paths.allow {
            unsafe {
                let _ = nano_sandbox::acl::ensure_allow_write_aces(p, &[sid]);
            }
        }
    });

    let (_, overall) = phase("spawn (pipes, cmd /c exit 0)", overall, || {
        let handles = nano_sandbox::process::spawn_process_with_pipes(
            security.h_token,
            &["cmd.exe".into(), "/c".into(), "exit 0".into()],
            &workspace,
            &std::collections::HashMap::new(),
            nano_sandbox::process::StdinMode::Closed,
            nano_sandbox::process::StderrMode::MergeStdout,
            nano_sandbox::process::ConsoleMode::Inherit,
            false,
            None,
        )
        .expect("spawn");
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                handles.process.hProcess,
                10_000,
            );
            windows_sys::Win32::Foundation::CloseHandle(handles.process.hProcess);
            windows_sys::Win32::Foundation::CloseHandle(handles.process.hThread);
        }
    });

    println!(
        "{:<28} {:>8.1} ms",
        "TOTAL",
        overall.elapsed().as_secs_f64() * 1000.0
    );

    let _ = std::fs::remove_dir_all(tmp.join("workspace").join("file.txt"));
    Ok(())
}
