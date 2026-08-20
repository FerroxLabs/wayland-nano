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
key-decisions:
  - "Keep operational failure strings static and bounded so argv and environment values cannot leak."
  - "Declare CreateJobObjectW directly because the authorized windows-sys feature floor intentionally excludes Win32_Security."
duration: 18min
completed: 2026-08-17
status: complete
---

# Phase 4 Plan 04: Contained Gate Runner Summary

## Behavioral RED Evidence

- **Command:** `$env:TEMP='F:\Temp\Codex'; $env:TMP='F:\Temp\Codex'; $env:CARGO_TARGET_DIR='F:\CargoTarget\wayland-nano'; cargo test -p nano-verify --test gate_contract`
- **Exit code:** `101`
- **Compilation/setup:** passed; the integration target compiled and ran six discovered tests (one fixture entry plus the five required public behaviors).
- **Failing tests:** `run_gate_parses_stdout_despite_nonzero_exit`, `run_gate_timeout_fails_closed`, `run_gate_spawn_error_fails_closed`, `run_gate_artifact_path_is_final_argv`, `run_gate_env_baseline_allowlist`.
- **Bounded assertion excerpt:** `left: FailClosed(NoGateOutput), right: FailClosed(Timeout)`; the other four named assertions likewise received the deliberate `NoGateOutput` seam instead of their required runner outcomes.

## Accomplishments

- Added an argv-only Tokio runner that appends the artifact as the final token, clears ambient environment state, restores only the exact IFACE §3 platform baseline, then overlays declared values.
- Streams and drains stdout while retaining at most 16 MiB, parses captured output without consulting child exit status, and maps every operational failure to a bounded fail-closed result.
- Starts Windows gates suspended, assigns them to a no-breakaway kill-on-close Job Object before resume, and terminates/reaps the complete job on timeout. Unix uses a new process group and group SIGKILL before reap.
- Added the five named cross-platform real-process tests, including a Windows process-state oracle proving the descendant is dead and an embedded output-cap exercise.

## Task Commits

1. **Task 1: Materialize subprocess fixture contract** — `5dba65e`
2. **Task 2: Implement bounded contained runner** — `3faaa4f`

## Verification

- `cargo fmt --all -- --check` — passed.
- `cargo clippy -p nano-verify --all-targets -- -D warnings` — passed.
- `cargo test -p nano-verify --test gate_contract` — passed: 6/6 (five named behaviors plus fixture entry).
- `cargo test -p nano-verify` — passed: 22/22 across library and integration targets.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Used a direct Windows API declaration for job creation**
- **Found during:** Task 2 compilation
- **Issue:** windows-sys 0.52 gates `CreateJobObjectW` behind `Win32_Security`, while Plan 04-01 intentionally authorized only Foundation, Storage, JobObjects, and Threading and Cargo changes were outside this plan.
- **Fix:** Declared the stable `kernel32!CreateJobObjectW` ABI locally while continuing to use the authorized windows-sys Job Object and Threading types/functions.
- **Files modified:** `crates/nano-verify/src/gate.rs`
- **Commit:** `3faaa4f`

## Known Stubs

None.

## Self-Check: PASSED

- Both owned code artifacts exist and all task commits are present.
- The implementation contains no placeholder/TODO/FIXME paths and the five required tests are discovered and green.
