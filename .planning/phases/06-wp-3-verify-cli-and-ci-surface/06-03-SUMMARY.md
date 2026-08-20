---
phase: 06-wp-3-verify-cli-and-ci-surface
plan: 03
subsystem: cli
tags: [rust, receipts, git-worktree, offline-verification, fail-closed]
requires:
  - phase: 06-wp-3-verify-cli-and-ci-surface
    provides: closed verify modes, runtime seam, registry loading, and bounded gate execution
provides:
  - Locked parse-to-preflight receipt verdict classification
  - Bounded detached fix-commit gate rerun
  - Unconditional remove/prune/filesystem-and-registration cleanup proof
affects: [06-06, 06-09, 06-10, CLI-04]
tech-stack:
  added: []
  patterns: [imported receipt contract, bounded Git subprocess, cleanup-overrides-success]
key-files:
  created: [.planning/phases/06-wp-3-verify-cli-and-ci-surface/06-03-SUMMARY.md]
  modified: [crates/nano-cli/src/verify_cmd.rs]
key-decisions:
  - "Treat structurally valid log_digest changes as provenance that offline verification cannot recompute."
  - "Reserve a bounded cleanup slice after verification timeout so cleanup remains mandatory and can override success."
patterns-established:
  - "Receipt validity requires imported WP2 Ready plus an actual Green rerun at fix_commit."
  - "Detached worktree cleanup is remove --force, prune, filesystem absence, then porcelain registration absence."
requirements-completed: [CLI-01, CLI-04, CLI-05]
coverage:
  - id: D1
    description: Locked offline receipt preflight and canonical verdict serialization
    requirement: CLI-04
    verification:
      - kind: unit
        ref: "verify_cmd::tests::receipt_preflight"
        status: pass
    human_judgment: false
  - id: D2
    description: Bounded detached rerun with fail-closed cleanup on every injected outcome
    requirement: CLI-04
    verification:
      - kind: integration
        ref: "verify_cmd::tests::receipt_rerun"
        status: pass
      - kind: other
        ref: "just gate-all"
        status: pass
    human_judgment: false
duration: 22min
completed: 2026-08-21
status: complete
---

# Phase 6 Plan 3: Offline Receipt Verification Summary

**Offline receipts now become valid only after the locked imported preflight and a bounded Green rerun in a proven-clean detached fix-commit worktree.**

## Performance

- **Duration:** 22 min
- **Completed:** 2026-08-21
- **Tasks:** 2 TDD tasks
- **Files modified:** 2

## Accomplishments

- Enforced parse/schema/red-evidence classification before Git or registry work and mapped every imported preflight terminal to the exact closed verdict.
- Reconstructed the pinned registry invocation only after `ReceiptPreflight::Ready`, bounded it with `NANO_VERIFY_RECEIPT_BUDGET_MS`, and mapped Green to valid/0, Red to gate-mismatch/6, and every operational failure to unverifiable/6.
- Made detached worktree removal unconditional and fail-closed, including `remove --force`, `prune`, filesystem absence, and `git worktree list --porcelain` absence proof.

## Task Commits

1. **Task 1 RED:** `211566d` — receipt preflight failure cases.
2. **Task 1 GREEN:** `4fb8392` — locked receipt preflight order and verdict output.
3. **Task 2 RED:** `59eeff9` — detached rerun and cleanup failure matrix.
4. **Task 2 GREEN:** `1b647fa` — bounded rerun and unconditional cleanup implementation.

## Files Created/Modified

- `crates/nano-cli/src/verify_cmd.rs` — offline receipt classifier, bounded detached rerun, verdict serialization, and cleanup guard.
- `.planning/phases/06-wp-3-verify-cli-and-ci-surface/06-03-SUMMARY.md` — execution evidence and traceability.

## Decisions Made

- A missing/unreadable receipt remains usage exit 2; every readable non-valid outcome is exit 6.
- Unknown fields and unknown schema are unverifiable; malformed or incomplete red evidence is never-red.
- A changed but structurally valid `log_digest` cannot be detected offline and is explicitly treated as provenance, not proof.
- Cleanup receives a finite post-verification slice even after timeout and overrides any provisional valid result if absence cannot be proven.

## Verification

- `cargo test -p nano-cli verify_cmd::tests::receipt_preflight --lib -- --nocapture` — 2 passed.
- `cargo test -p nano-cli verify_cmd::tests::receipt_rerun --lib -- --nocapture` — 4 passed, including a real detached Git worktree lifecycle.
- `cargo test -p nano-cli verify_cmd::tests --lib -- --nocapture` — 19 passed.
- `cargo clippy -p nano-cli --all-targets -- -D warnings` — passed.
- `just gate-all` with F:-only TEMP/TMP/CARGO_TARGET_DIR — passed.

## Deviations from Plan

None - plan executed exactly as written.

## Known Stubs

None in the Plan 03 receipt verification surface.

## Threat Flags

| Flag | File | Description |
|---|---|---|
| threat_flag: bounded-local-process | `crates/nano-cli/src/verify_cmd.rs` | Local Git worktree probes are bounded, offline, argv-only, and fail closed. |

## Self-Check: PASSED

- Product file and summary exist.
- All four TDD commits exist on `worktree-agent-wp3-03`.
- Full workspace gate passed on final product bytes.
- No file outside the assigned Plan 03 product and summary paths changed.

