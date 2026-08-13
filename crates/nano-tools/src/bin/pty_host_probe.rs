//! External host-kill probe for the persistent PTY job lifetime.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use nano_tools::pty::{PtyReadRequest, PtySessionManager, PtySpawnRequest};
    use std::io::Write;
    use std::time::{Duration, Instant};

    let workspace = std::env::current_dir()?;
    let manager = PtySessionManager::new(&workspace);
    let spawned = manager.spawn(PtySpawnRequest {
        // Starts ping, reports its PID, then waits forever. EncodedCommand
        // avoids cmd.exe nested-quote rewriting in this external proof.
        command: "%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe -NoProfile -EncodedCommand JABwAD0AUwB0AGEAcgB0AC0AUAByAG8AYwBlAHMAcwAgAHAAaQBuAGcALgBlAHgAZQAgAC0AQQByAGcAdQBtAGUAbgB0AEwAaQBzAHQAIAAnAC0AdAAnACwAJwAxADIANwAuADAALgAwAC4AMQAnACAALQBQAGEAcwBzAFQAaAByAHUAOwAgAFcAcgBpAHQAZQAtAE8AdQB0AHAAdQB0ACAAKAAnAEQASQBSAEUAQwBUAF8AUABJAEQAPQAnACAAKwAgACQAcAAuAEkAZAApADsAIABXAGEAaQB0AC0AUAByAG8AYwBlAHMAcwAgACQAcAAuAEkAZAA=".into(),
        cwd: None,
    })?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut offset = 0;
    let mut output = String::new();
    loop {
        let read = manager.read(PtyReadRequest {
            session_id: spawned.session_id.clone(),
            after_offset: Some(offset),
            yield_time_ms: Some(250),
            max_bytes: None,
        })?;
        offset = read.next_offset;
        output.push_str(&read.chunk);
        if let Some(marker) = output.find("DIRECT_PID=") {
            let digits: String = output[marker + 11..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if !digits.is_empty() {
                println!("DIRECT_PID={digits}");
                std::io::stdout().flush()?;
                std::thread::sleep(Duration::from_secs(120));
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(format!("direct child PID not reported: {output:?}").into());
        }
    }
}

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use nano_tools::pty::{PtySessionManager, PtySpawnRequest};

    let workspace = std::env::current_dir()?;
    let manager = PtySessionManager::new(&workspace);
    // A long-sleep session: the guard (this process's direct descendant) and
    // the sleep in its process group must not survive an abrupt host death.
    // The test performs the process inventory externally (pgrep/ps), so this
    // probe only needs to keep the session alive and signal readiness.
    manager.spawn(PtySpawnRequest {
        command: "sleep 600".into(),
        cwd: None,
    })?;
    println!("READY");
    std::io::Write::flush(&mut std::io::stdout())?;
    std::thread::sleep(std::time::Duration::from_secs(120));
    Ok(())
}

#[cfg(not(any(windows, unix)))]
fn main() {}
