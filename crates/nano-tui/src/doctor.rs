//! `/status` + `/doctor` data path (normative, panel condition C1): both
//! run `wayland-nano doctor` as a SHORT-LIVED SUBPROCESS — the same trust
//! boundary as the acp-host spawn. The TUI never links nano-cli and never
//! reimplements doctor's probes.
//!
//! doctor has no JSON mode in v1 (nano-cli/src/doctor.rs emits plain
//! `PASS/WARN/FAIL name detail` lines plus a `summary:` line), so the
//! output is parsed CONSERVATIVELY: the full text renders verbatim
//! (sanitized); the only extracted field is the `summary:` line for the
//! status bar, taken only when it matches the exact expected shape.
//! Anything unexpected renders as plain text — no fragile field scraping.

use std::process::Command;

use crate::event::AppEvent;
use crate::event::AppEventSender;

/// Locate the `wayland-nano` binary: NANO_EXE override, else next to the
/// current exe (the target-dir sibling), else PATH.
pub fn wayland_nano_program() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("NANO_EXE") {
        return std::path::PathBuf::from(path);
    }
    if let Ok(here) = std::env::current_exe()
        && let Some(dir) = here.parent()
    {
        let sibling = dir.join(format!("wayland-nano{}", std::env::consts::EXE_SUFFIX));
        if sibling.exists() {
            return sibling;
        }
    }
    std::path::PathBuf::from(format!("wayland-nano{}", std::env::consts::EXE_SUFFIX))
}

/// Run doctor on a worker thread (short-lived subprocess; the UI loop stays
/// responsive) and post the result as an AppEvent.
pub fn run_doctor_async(sender: AppEventSender, nano_home: &std::path::Path) {
    let program = wayland_nano_program();
    let nano_home = nano_home.to_path_buf();
    std::thread::spawn(move || {
        let (output, exit_code) = match Command::new(&program)
            .arg("doctor")
            .env("NANO_HOME", &nano_home)
            .output()
        {
            Ok(out) => {
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                if !out.stderr.is_empty() {
                    text.push_str(&String::from_utf8_lossy(&out.stderr));
                }
                (text, out.status.code().unwrap_or(-1))
            }
            Err(err) => (format!("failed to run {}: {err}", program.display()), 2),
        };
        sender.send(AppEvent::DoctorDone { output, exit_code });
    });
}

/// Conservative summary extraction: the exact `summary: N fail, M warn`
/// line doctor prints, or None (never guesses).
pub fn summary_line(doctor_output: &str) -> Option<String> {
    doctor_output.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("summary:")?;
        let mut parts = rest.trim().split(", ");
        let fail = parts
            .next()?
            .strip_suffix("fail")?
            .trim()
            .parse::<u32>()
            .ok()?;
        let warn = parts
            .next()?
            .strip_suffix("warn")?
            .trim()
            .parse::<u32>()
            .ok()?;
        parts
            .next()
            .is_none()
            .then(|| format!("{fail} fail, {warn} warn"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "wayland-nano doctor — 0.1.0\n  PASS  os                       windows / x86_64\n  WARN  sandbox-provisioning     not provisioned\n  PASS  egress-policy            non-allowlisted hosts denied at construction\nsummary: 0 fail, 1 warn\n";

    #[test]
    fn extracts_exact_summary() {
        assert_eq!(summary_line(SAMPLE), Some("0 fail, 1 warn".to_string()));
    }

    #[test]
    fn refuses_lookalikes() {
        assert_eq!(summary_line("summary: soon"), None);
        assert_eq!(summary_line("summary: 0 fail, 1 warn, extra"), None);
        assert_eq!(summary_line("no summary here"), None);
    }
}
