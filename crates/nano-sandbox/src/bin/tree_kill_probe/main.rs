//! wayland-nano-tree-kill-probe — C1.2 probe: spawn a real process tree under a Job
//! Object and prove terminate kills parent + descendants, fast.
//!
//! Prints `TREE_KILL_OK ms=<elapsed>` when the whole tree is gone after
//! `job.terminate()`, exits nonzero otherwise. External verification (no
//! survivors) is performed by the PowerShell harness via CIM — this binary's
//! own report is NOT the oracle.

#[cfg(target_os = "windows")]
mod win {
    use nano_sandbox::job::JobObject;
    use std::time::Instant;
    use tokio::process::Command;

    #[tokio::main(flavor = "current_thread")]
    pub(crate) async fn main() -> anyhow::Result<()> {
        let job = JobObject::create()?;

        // cmd spawns a sleeping descendant; the job must kill both.
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/c", "ping -t 127.0.0.1 > NUL"]);
        let mut child = job.spawn_contained(&mut cmd)?;
        let pid = child.id().unwrap_or(0);
        println!("spawned_tree_root_pid={pid}");

        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        let started = Instant::now();
        job.terminate()?;
        let status = child.wait().await?;
        let elapsed = started.elapsed();

        println!("TREE_KILL_OK ms={} exit={}", elapsed.as_millis(), status);
        if elapsed.as_millis() > 5000 {
            anyhow::bail!("tree kill exceeded 5000ms");
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    win::main()
}

#[cfg(not(target_os = "windows"))]
fn main() {
    panic!("wayland-nano-tree-kill-probe is Windows-only");
}
