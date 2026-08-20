---
phase: 04-wp-1-gate-and-receipt-foundation
plan: 03
subsystem: verification
tags: [rust, parser, fail-closed, tdd]
requires:
  - phase: 04-wp-1-gate-and-receipt-foundation
    provides: nano-verify crate and gate module seam
provides:
  - Contract-verbatim gate invocation and outcome types
  - Pure fail-closed gate-output parser with full inventory reconstruction
  - Canonical score and opaque failure-key projections
affects: [wp-1-gate-runner, wp-2-climb, wp-3-receipt-verification]
tech-stack:
  added: []
  patterns: [hand-rolled bounded scanner, authoritative inventory reconstruction, assertion-derived TDD]
key-files:
  created: []
  modified: [crates/nano-verify/src/gate.rs]
key-decisions:
  - "Malformed, unknown, duplicate, or category-conflicting FAIL records fail closed and never pass through attacker-controlled text via fails()."
  - "The last syntactically valid summary is authoritative only when its totals agree with the complete nonempty inventory and unique failures."
patterns-established:
  - "Gate parsing is pure and uses no regex or subprocess behavior."
  - "FailClosed projections expose fixed bounded sentinels; Red projections expose only canonical inventory ID/category pairs."
requirements-completed: [GATE-02, GATE-03, RCPT-04]
coverage:
  - id: D1
    description: "Gate text is parsed with last-summary semantics and complete inventory reconstruction"
    requirement: GATE-02
    verification:
      - kind: unit
        ref: "cargo test -p nano-verify gate::tests"
        status: pass
    human_judgment: false
  - id: D2
    description: "Ambiguous, missing, unknown, empty, or inconsistent output fails closed with opaque projections"
    requirement: GATE-03
    verification:
      - kind: unit
        ref: "crates/nano-verify/src/gate.rs#nine named parser tests"
        status: pass
    human_judgment: false
  - id: D3
    description: "Failure projections contain only canonical ID/category keys or bounded sentinels"
    requirement: RCPT-04
    verification:
      - kind: unit
        ref: "cargo clippy -p nano-verify --all-targets -- -D warnings; cargo test -p nano-verify gate::tests"
        status: pass
    human_judgment: false
duration: 12min
completed: 2026-08-17
status: complete
---

# Phase 4 Plan 03: Canonical Gate Output Parser Summary

**Pure fail-closed gate parser with overflow-safe summary scanning, authoritative full-inventory verdict reconstruction, and opaque score/failure projections**

## Performance

- **Duration:** 12 min
- **Started:** 2026-08-17T22:51:00+07:00
- **Completed:** 2026-08-17T23:03:32+07:00
- **Tasks:** 1 TDD feature
- **Files modified:** 2

## Accomplishments

- Copied the IFACE gate types and implemented the pure parser without regex or subprocess behavior.
- Enforced closed failure grammar, checked integer parsing, last-valid-summary selection, known-ID resolution, unique failure accounting, nonempty inventory, and exact summary equations.
- Added all nine SPEC-WP12 parser tests, including exact outcomes, inventory order, score tuples, and opaque failure keys.

## TDD Evidence

### RED

- **Exact command:** `$env:TEMP='F:\Temp\Codex'; $env:TMP='F:\Temp\Codex'; $env:CARGO_TARGET_DIR='F:\CargoTarget\wayland-nano'; cargo test -p nano-verify gate::tests::parse_`
- **Exit code:** `101`
- **Cause:** behavioral assertion failures against compiling deliberately-wrong `Timeout` / `<gate parser unavailable>` stubs; compilation and test setup succeeded.
- **Failing tests:** `parse_empty_stdout_fails_closed`, `parse_fail_v2_canonical`, `parse_fail_v2_whitespace_collapses`, `parse_no_summary_fails_closed`, `parse_prefixed_slug_summary`, `parse_reconstructs_full_verdict_inventory`, `parse_summary_last_match_wins`, `parse_unknown_fail_id_fails_closed`.
- **Bounded assertion excerpt:** `left: FailClosed(Timeout)` / `right: FailClosed(NoGateOutput)` and `left: ["<gate parser unavailable>"]` / `right: ["TG-03 structure"]`.
- The ninth required test is named `summary_inventory_mismatch_fails_closed`, so the plan-mandated `gate::tests::parse_` filter intentionally excludes it; the full module run below executes all nine.

### GREEN

- The exact RED command passed: `8 passed; 0 failed; 1 filtered out`.
- `cargo test -p nano-verify gate::tests` passed: `9 passed; 0 failed`.
- `cargo clippy -p nano-verify --all-targets -- -D warnings` passed.

## Task Commits

1. **RED: compiling contract types, deliberately-wrong stubs, and behavioral assertions** — `8f63aaf`
2. **GREEN/REFACTOR: auditable fail-closed parser and projections** — `8d65612`

## Files Created/Modified

- `crates/nano-verify/src/gate.rs` — contract types, pure parser, projections, scanners, and nine unit tests.
- `.planning/phases/04-wp-1-gate-and-receipt-foundation/04-03-SUMMARY.md` — execution and TDD evidence.

## Decisions Made

- Exact `FAIL ` recognition is retained; subsequent token whitespace collapses through `split_whitespace`.
- Duplicate failure IDs and category conflicts fail closed as inconsistent output instead of inflating failure counts.
- Failure reason payloads remain available for typed diagnosis, while `fails()` maps them to fixed sentinels so ambient or attacker-controlled text cannot escape.

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None. The deliberately-wrong RED stubs were replaced before completion.

## Issues Encountered

The mandated `parse_` test filter selects eight of the nine contract tests because the specified mismatch test name lacks that prefix. Both the exact filtered command and the full nine-test module command were run and recorded.

## User Setup Required

None — no external services or secrets are used.

## Next Phase Readiness

The pure parser/types/score/fails trust anchor is ready for the separately owned subprocess runner. No subprocess seam was implemented in this plan.

## Self-Check: PASSED

- `crates/nano-verify/src/gate.rs` exists and contains all nine required named tests.
- Commits `8f63aaf` and `8d65612` exist.
- Exact filtered verification, full nine-test verification, formatting, and warning-free clippy passed.

---
*Phase: 04-wp-1-gate-and-receipt-foundation*
*Completed: 2026-08-17*
