---
phase: 06-wp-3-verify-cli-and-ci-surface
plan: 02
subsystem: cli
tags: [rust, jsonl, deadline, registry, run-only]
requires:
  - phase: 06-wp-3-verify-cli-and-ci-surface
    provides: closed verify parser and nano-verify imports
provides:
  - Closed bounded JSONL v1 verify event sink
  - Injectable clock, generation, Git, filesystem, registry, inventory, and gate seams
  - F-only run-only preflight and one checked absolute deadline
  - Registry-backed confined run-only gate execution with exact 0/2/3 mapping
affects: [06-03, 06-04, 06-05, 06-10]
tech-stack:
  added: []
  patterns: [closed serializer, checked absolute deadline, injected effects boundary]
key-files:
  created: [.planning/phases/06-wp-3-verify-cli-and-ci-surface/06-02-SUMMARY.md]
  modified: [crates/nano-cli/src/verify_cmd.rs]
key-decisions:
  - "Treat Windows verbatim-drive canonical paths as F: while comparing TEMP/TMP by their canonical targets."
  - "Keep receipt-check post-read behavior fail-closed at Unverifiable/6 until Plan 03 installs the locked verification pipeline."
metrics:
  tasks: 2
  files: 2
completed: 2026-08-21
status: complete
---

# Phase 6 Plan 2: Closed Events and Run-Only Summary

**Schema-closed verify telemetry and a registry-backed run-only path governed by one non-resetting monotonic deadline.**

## Accomplishments

- Added all eight allowed verify event types, fixed-code errors, process-local sequence discipline, and identifier-only verdict/climb/receipt payloads.
- Added an internal effects seam covering clock, generation, Git/root discovery, TEMP/TMP, registry/filesystem resolution, inventory, and gate execution while leaving public `run` generic-free.
- Implemented F:-resident repository and canonical matching TEMP/TMP preflight, confined run artifacts, closure-derived invocation, exact remaining-millisecond timeout narrowing, and no-spawn-on-expiry behavior.
- Preserved receipt-check entry ordering: unreadable receipt files return usage 2 before repository/temp checks; readable inputs remain Unverifiable/6 for Plan 03 to complete.

## Task Commits

1. `d6b0d0f` — RED event schema/leakage tests.
2. `fd22d8d` — GREEN closed JSONL event sink.
3. `7e6834e` — RED run-only/deadline/classification tests.
4. `e36a929` — GREEN bounded run-only implementation and injected seams.

## Verification

- `cargo test -p nano-cli verify_cmd::tests::events --lib -- --nocapture` — pass.
- `cargo test -p nano-cli verify_cmd::tests::run_only --lib -- --nocapture` — pass.
- `cargo test -p nano-cli verify_cmd::tests::deadline --lib -- --nocapture` — pass.
- `cargo test -p nano-cli verify_cmd::tests::receipt_entry_classification --lib -- --nocapture` — pass.
- `cargo test -p nano-cli verify_cmd::tests --lib` — 12 passed.
- `cargo clippy -p nano-cli --all-targets -- -D warnings` — pass.
- `just gate-all` under F:-only TEMP/TMP/CARGO_TARGET_DIR — pass.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Accepted canonical Windows verbatim drive spelling**

- **Found during:** Task 2 GREEN
- **Issue:** `Path::canonicalize` produces a verbatim disk prefix on Windows, so byte-comparing it to a conventional absolute `F:\...` environment spelling incorrectly rejected a valid canonical target.
- **Fix:** Reject relative/dot-component environment values, canonicalize both TEMP and TMP, compare their canonical targets, and recognize both Disk and VerbatimDisk F: prefixes.
- **Files modified:** `crates/nano-cli/src/verify_cmd.rs`
- **Commit:** `e36a929`

## Known Stubs

- `crates/nano-cli/src/verify_cmd.rs`: minting stops fail-closed after the Plan 02 entry gate; climb/materialization belongs to later WP-3 plans.
- `crates/nano-cli/src/verify_cmd.rs`: readable receipt verification returns Unverifiable/6 until Plan 03 installs the required locked-order verifier.

## Self-Check: PASSED

- Product file and summary exist.
- All four task commits exist on `worktree-agent-wp3-02`.
- No files outside the assigned Plan 02 product and summary paths were changed.
