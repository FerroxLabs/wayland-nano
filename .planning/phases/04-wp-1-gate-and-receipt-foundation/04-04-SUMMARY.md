---
phase: 04-wp-1-gate-and-receipt-foundation
plan: 04
subsystem: verification
tags: [rust, tokio, subprocess, process-tree, fail-closed]
requires:
  - phase: 04-wp-1-gate-and-receipt-foundation
    provides: Gate output parser and canonical invocation seam
provides:
  - Contained, environment-scrubbed, bounded gate subprocess execution
affects: [wp-2-engine, wp-3-cli]
tech-stack:
  added: []
  patterns: [argv-only spawn, bounded stdout, whole-tree timeout]
key-files:
  created: [crates/nano-verify/tests/gate_contract.rs]
  modified: [crates/nano-verify/src/gate.rs]
key-decisions: []
duration: pending
completed: 2026-08-17
status: in-progress
---

# Phase 4 Plan 04: Contained Gate Runner Summary

## Behavioral RED Evidence

- **Command:** `$env:TEMP='F:\Temp\Codex'; $env:TMP='F:\Temp\Codex'; $env:CARGO_TARGET_DIR='F:\CargoTarget\wayland-nano'; cargo test -p nano-verify --test gate_contract`
- **Exit code:** `101`
- **Compilation/setup:** passed; the integration target compiled and ran six discovered tests (one fixture entry plus the five required public behaviors).
- **Failing tests:** `run_gate_parses_stdout_despite_nonzero_exit`, `run_gate_timeout_fails_closed`, `run_gate_spawn_error_fails_closed`, `run_gate_artifact_path_is_final_argv`, `run_gate_env_baseline_allowlist`.
- **Bounded assertion excerpt:** `left: FailClosed(NoGateOutput), right: FailClosed(Timeout)`; the other four named assertions likewise received the deliberate `NoGateOutput` seam instead of their required runner outcomes.

