---
phase: 04-wp-1-gate-and-receipt-foundation
plan: 06
subsystem: verification
tags: [rust, git, receipt, preflight, fail-closed]
requires:
  - phase: 04-wp-1-gate-and-receipt-foundation
    provides: canonical receipt storage and canonical registry authority
provides:
  - Bounded scrubbed system-Git receipt probes
  - Normative schema, red, commit, ancestry, path, mapping, and pin preflight
  - Materialized nine-test receipt Git fixture battery
affects: [wp-3-final-receipt-verification]
tech-stack:
  added: []
  patterns: [fixed-argv Git probes, three-state external evidence, preflight-only Ready]
key-files:
  created: [crates/nano-verify/tests/receipt_git.rs]
  modified: [crates/nano-verify/src/receipt.rs]
key-decisions:
  - "Treat spawn, timeout, reap, and unexpected probe failures as Unverifiable while absent Git objects remain FabricatedCommit."
  - "Keep Ready structurally separate from VerifyVerdict and reserve detached worktrees, gate reruns, and Valid for WP-3."
requirements-completed: [RCPT-01, RCPT-03, RCPT-04]
duration: 12min
completed: 2026-08-17
status: complete
---

# Phase 4 Plan 06: Receipt Git Preflight Summary

**Read-only receipt preflight with three-second scrubbed Git probes and a complete red-before-green evidence chain**

## Accomplishments

- Materialized repositories at test runtime with local commit identity flags, related and unrelated commits, a blob object, and test-present/test-absent trees.
- Implemented fixed-argument system-Git probes with configuration and credential-helper controls, inherited repository overrides removed, 25 ms polling, and kill/reap at three seconds.
- Enforced normative preflight order: strict parse/schema, genuine red evidence, both commit peels, ancestry, test object at observed commit, requirement mapping, and recomputed closure pin.
- Preserved the WP-1 boundary: success is only `ReceiptPreflight::Ready`; no production path creates a worktree, reruns a gate, or emits final `VerifyVerdict::Valid`.

## Behavioral RED Evidence

- **Exact command:** `$env:TEMP='F:\Temp\Codex'; $env:TMP='F:\Temp\Codex'; $env:CARGO_TARGET_DIR='F:\CargoTarget\wayland-nano'; cargo fmt --all; cargo test -p nano-verify --test receipt_git`
- **Exit code:** `101`
- **Bounded result:** `3 passed; 6 failed; 0 ignored`
- **Failing tests:** `mint_and_preflight_ready_in_repo`, `preflight_fabricated_commit`, `preflight_gate_mismatch_digest_drift`, `preflight_gate_mismatch_unknown_or_unmapped_gate`, `preflight_never_red_on_zero_exit`, `preflight_rejects_unproven_ancestry_or_test_path`
- **Assertion excerpt:** `left: Unverifiable` versus the required `Ready`, `FabricatedCommit`, `GateMismatch`, `NeverRed`, or `AncestryUnproven` arm.
- **RED validity:** all nine tests compiled and all dynamic repositories initialized successfully. The three cases expecting `Unverifiable` passed; failures were exclusively the named missing behaviors behind the compiling preflight seam.

## Task Commits

1. **Task 1 RED: materialized receipt Git fixture battery** — `e5718c1`
2. **Task 2 GREEN: bounded normative receipt preflight** — `833a208`

## Deviations from Plan

None - plan executed exactly as written.

## Known Stubs

None.

## Threat Mitigations

- **T-04-16:** both receipt SHAs must peel to commits; observed must precede fix; the receipt test object must exist at observed.
- **T-04-17:** fixed argv without a shell, scrubbed Git configuration/repository/credential variables, and bounded child termination.
- **T-04-18:** registry requirement identity, gate identity, stored pin, and independently recomputed closure digest must agree.

## Verification

- `cargo test -p nano-verify --test receipt_git` — 9 passed.
- `cargo clippy -p nano-verify --test receipt_git -- -D warnings` — passed.
- `cargo test -p nano-verify` — 31 passed across unit and integration targets.
- `cargo fmt --all -- --check` — passed.
- Structural scan found no production worktree or gate-run path; the sole `VerifyVerdict::Valid` reference is a negative type-separation assertion.

## Self-Check: PASSED

- Both implementation files and this summary exist.
- Commits `e5718c1` and `833a208` exist.
- No generated files, goal-blocking stubs, or unmodeled security surface remain.

---
*Phase: 04-wp-1-gate-and-receipt-foundation*
*Completed: 2026-08-17*
