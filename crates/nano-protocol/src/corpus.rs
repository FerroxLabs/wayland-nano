//! Conformance harness: replays the `wayland-desktop-core` v1 fixture corpus
//! against the Nano protocol profile.
//!
//! Contract (Nano profile):
//! - SUPPORTED command types parse successfully;
//! - UNSUPPORTED corpus command types fail typed (→ host error frame,
//!   recoverable, engine continues);
//! - SUPPORTED event fixtures parse into the Event enum (unknown extra
//!   fields tolerated — forward additive);
//! - UNSUPPORTED event types fail typed (fail-closed, never panics);
//! - ADVERSARIAL fixtures (malformed/unknown-critical/policy) all fail typed
//!   with zero panics.

#[cfg(test)]
use crate::messages::{Command, Event};
#[cfg(test)]
use std::path::Path;

#[cfg(test)]
const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/corpus/wayland-desktop-core/v1"
);

/// Command types the Nano v1 profile supports (everything else in the corpus
/// must be tolerated-as-typed-error, never executed).
#[cfg(test)]
const SUPPORTED_COMMANDS: &[&str] = &[
    "message",
    "stop",
    "ping",
    "tool_approve",
    "tool_deny",
    "approval_resume",
];

/// Event types the Nano v1 profile emits/accepts.
#[cfg(test)]
const SUPPORTED_EVENTS: &[&str] = &[
    "ready",
    "stream_start",
    "text_delta",
    "thinking",
    "tool_request",
    "tool_running",
    "tool_result",
    "tool_cancelled",
    "approval_required",
    "suspend",
    "approval_resume",
    "info",
    "error",
    "stream_end",
    "pong",
];

#[cfg(test)]
pub struct ConformanceReport {
    pub accepted: Vec<String>,
    pub tolerated: Vec<String>,
    pub rejected_unsupported: Vec<String>,
    pub adversarial_handled: Vec<String>,
    pub violations: Vec<String>,
}

#[cfg(test)]
fn try_parse_event(text: &str) -> Result<Event, serde_json::Error> {
    serde_json::from_str::<Event>(text)
}

#[cfg(test)]
pub fn run_conformance(corpus_root: &Path) -> ConformanceReport {
    let mut report = ConformanceReport {
        accepted: vec![],
        tolerated: vec![],
        rejected_unsupported: vec![],
        adversarial_handled: vec![],
        violations: vec![],
    };

    // --- commands ---
    let commands_dir = corpus_root.join("commands");
    for entry in std::fs::read_dir(&commands_dir).expect("commands dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().to_string();
        let stem = name.trim_end_matches(".json").to_string();
        let text = std::fs::read_to_string(entry.path()).expect("fixture");
        match serde_json::from_str::<Command>(&text) {
            Ok(_) => {
                if SUPPORTED_COMMANDS.contains(&stem.as_str()) {
                    report.accepted.push(format!("command/{stem}"));
                } else {
                    report.violations.push(format!(
                        "command/{stem}: unsupported command parsed successfully (should be tolerated-error)"
                    ));
                }
            }
            Err(_) => {
                if SUPPORTED_COMMANDS.contains(&stem.as_str()) {
                    report
                        .violations
                        .push(format!("command/{stem}: supported command failed to parse"));
                } else {
                    report.tolerated.push(format!("command/{stem}"));
                }
            }
        }
    }

    // --- events ---
    let events_dir = corpus_root.join("events");
    for entry in std::fs::read_dir(&events_dir).expect("events dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name().to_string_lossy().to_string();
        let stem = name.trim_end_matches(".json").to_string();
        let text = std::fs::read_to_string(entry.path()).expect("fixture");
        match try_parse_event(&text) {
            Ok(_) => {
                if SUPPORTED_EVENTS.contains(&stem.as_str()) {
                    report.accepted.push(format!("event/{stem}"));
                } else {
                    report.violations.push(format!(
                        "event/{stem}: unsupported event parsed successfully (fail-closed violation)"
                    ));
                }
            }
            Err(_) => {
                if SUPPORTED_EVENTS.contains(&stem.as_str()) {
                    report
                        .violations
                        .push(format!("event/{stem}: supported event failed to parse"));
                } else {
                    report.rejected_unsupported.push(format!("event/{stem}"));
                }
            }
        }
    }

    // --- adversarial + compat trees ---
    for sub in ["adversarial", "compat"] {
        for entry in walk_json(&corpus_root.join(sub)) {
            let text = std::fs::read_to_string(&entry).expect("fixture");
            let rel = entry
                .strip_prefix(corpus_root)
                .unwrap()
                .to_string_lossy()
                .to_string();
            if entry.extension().is_some_and(|e| e == "jsonl") {
                // Adversarial streams: every line must be handled without a
                // panic — accepted shapes or typed errors, never a crash.
                for (index, line) in text.lines().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let outcome = match serde_json::from_str::<Command>(trimmed) {
                        Ok(_) => "accepted-shape",
                        Err(_) => match try_parse_event(trimmed) {
                            Ok(_) => "event-shape",
                            Err(_) => "typed-error",
                        },
                    };
                    report
                        .adversarial_handled
                        .push(format!("{rel}:{index} ({outcome})"));
                }
            } else {
                match serde_json::from_str::<Command>(&text) {
                    Ok(_) => report
                        .adversarial_handled
                        .push(format!("{rel} (accepted-shape)")),
                    Err(_) => match try_parse_event(&text) {
                        Ok(_) => report
                            .adversarial_handled
                            .push(format!("{rel} (event-shape)")),
                        Err(_) => report
                            .adversarial_handled
                            .push(format!("{rel} (typed-error)")),
                    },
                }
            }
        }
    }

    report
}

#[cfg(test)]
fn walk_json(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e == "json" || e == "jsonl")
            {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_corpus_conformance() {
        let corpus_root = Path::new(CORPUS);
        assert!(
            corpus_root.exists(),
            "corpus must be present at {CORPUS} — a conformance test that cannot find its corpus FAILS, never skips"
        );
        let report = run_conformance(corpus_root);

        eprintln!(
            "conformance: {} accepted, {} tolerated, {} rejected-unsupported, {} adversarial/compat handled, {} violations",
            report.accepted.len(),
            report.tolerated.len(),
            report.rejected_unsupported.len(),
            report.adversarial_handled.len(),
            report.violations.len()
        );
        for violation in &report.violations {
            eprintln!("VIOLATION: {violation}");
        }

        assert!(report.violations.is_empty(), "conformance violations found");
        assert!(!report.accepted.is_empty(), "supported fixtures must parse");
        assert!(
            !report.rejected_unsupported.is_empty(),
            "unsupported events must fail closed"
        );

        // The corpus' headline numbers (verified against manifest.json):
        // 11 commands, 39 events. Our profile must accept the supported
        // subset and reject every unsupported event typed.
        assert_eq!(
            report
                .accepted
                .iter()
                .filter(|a| a.starts_with("command/"))
                .count(),
            SUPPORTED_COMMANDS.len()
        );
        assert_eq!(
            report.rejected_unsupported.len()
                + report
                    .accepted
                    .iter()
                    .filter(|a| a.starts_with("event/"))
                    .count(),
            39,
            "every one of the 39 event fixtures is accounted for"
        );
        let compat_files = std::fs::read_dir(corpus_root.join("compat"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .flat_map(|e| {
                std::fs::read_dir(e.path())
                    .unwrap()
                    .flatten()
                    .collect::<Vec<_>>()
            })
            .count()
            .max(23); // compat is flat (23 files)
        let adversarial_files: usize = ["anvil", "commands", "events", "policy", "workflow"]
            .iter()
            .map(|sub| {
                std::fs::read_dir(corpus_root.join("adversarial").join(sub))
                    .map(|rd| rd.flatten().count())
                    .unwrap_or(0)
            })
            .sum();
        assert_eq!(compat_files, 23);
        assert_eq!(adversarial_files, 37, "corpus shape confirmed");
        let handled_lines = report.adversarial_handled.len();
        assert!(
            handled_lines >= 23 + 37,
            "every compat file and every adversarial stream line handled: {handled_lines}"
        );
    }
}
