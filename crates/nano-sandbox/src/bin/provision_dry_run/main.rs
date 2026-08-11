//! wayland-nano-provision-dry-run — owner-review tool for live provisioning.
//!
//! Prints the exact provisioning payload (pretty JSON) and the launch
//! command, WITHOUT executing anything. The privileged step is always a
//! separate, owner-run elevated invocation of wayland-nano-sandbox-setup.exe.

#[cfg(target_os = "windows")]
mod win {
    use nano_sandbox::setup_exec::WindowsSandboxProvisioningSettings;
    use nano_sandbox::setup_exec::provisioning_payload_review;

    pub(crate) fn main() -> anyhow::Result<()> {
        let nano_home = std::env::var_os("NANO_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| dirs_next::home_dir().expect("home dir").join(".nano"));
        let real_user = std::env::var("USERNAME").unwrap_or_else(|_| "unknown".into());
        let settings = WindowsSandboxProvisioningSettings {
            proxy_ports: Vec::new(),
            allow_local_binding: false,
        };

        let (pretty, b64) = provisioning_payload_review(&nano_home, &real_user, settings)?;

        println!("=== wayland-nano provisioning dry run (NOTHING EXECUTED) ===");
        println!("nano_home: {}", nano_home.display());
        println!("real_user: {real_user}");
        println!();
        println!("--- payload (what the elevated helper would receive) ---");
        println!("{pretty}");
        println!();
        println!("--- to execute (ELEVATED PowerShell, after review) ---");
        println!(r"target\release\wayland-nano-sandbox-setup.exe {b64}");
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    win::main()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    panic!("wayland-nano-provision-dry-run is Windows-only");
}
