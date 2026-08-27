---
phase: 03-wp-0.3-pdf-intake
plan: 02
subsystem: error-contract
tags: [rust, serde, json-rpc, pdf, generated-contract]
requires:
  - phase: 03-wp-0.3-pdf-intake
    provides: WP-0.3 ownership controls and provider catalog groundwork
provides:
  - Canonical model_lacks_pdf typed refusal
  - Exhaustive 71-kind error table
  - Generator-fresh Nano and canonical shared JSON mirrors
affects: [pdf-intake, cli-routing, cross-track-error-contract]
tech-stack:
  added: []
  patterns: [closed serde error vocabulary, generator-owned contract mirrors]
key-files:
  created: []
  modified:
    - crates/nano-session/src/error_kind.rs
    - crates/nano-session/src/error_codes.rs
    - crates/nano-protocol/src/error_codes.rs
    - crates/nano-cli/src/bin/gen_error_table.rs
    - crates/nano-session/contracts/nano-error-codes.json
    - D:/Development/waylandnano/shared/contracts/nano-error-codes.json
key-decisions:
  - "ModelLacksPdf is a non-retryable -32602 ErrorResponse selected only after leaf binding resolution."
  - "Both tracked mirrors remain generator-owned and Desktop generation is isolated to an absent temporary path."
patterns-established:
  - "New cross-process error kinds update the enum, exhaustive spec, ALL_KINDS, pinned tests, and generator mirrors together."
requirements-completed: [PDF-03, PDF-06]
coverage:
  - id: D1
    description: ModelLacksPdf has the exact stable wire code, presentation, retry policy, and 71-kind exhaustive-table membership.
    requirement: PDF-06
    verification:
      - kind: unit
        ref: "cargo test -p nano-session error_codes; cargo test -p nano-protocol error_codes"
        status: pass
    human_judgment: false
  - id: D2
    description: Nano and canonical shared error-code mirrors are byte-fresh generator output without a Desktop mirror write.
    requirement: PDF-06
    verification:
      - kind: integration
        ref: "cargo run -p nano-cli --bin gen_error_table -- --check under isolated NANO_ERROR_TABLE_DESKTOP_DIR"
        status: pass
      - kind: unit
        ref: "cargo test -p nano-cli --bin gen_error_table#canonical_shared_target_is_required_and_missing_fails_check"
        status: pass
      - kind: other
        ref: "03-OWNERSHIP-PREFLIGHT.ps1 -Mode Check"
        status: pass
    human_judgment: false
duration: 60min
completed: 2026-08-17
status: complete
---

# Phase 3 Plan 2: Canonical PDF Refusal Summary

**A stable non-retryable `model_lacks_pdf` refusal with exhaustive 71-kind coverage and byte-fresh generated Nano/shared contracts**

## Performance

- **Duration:** 60 min
- **Completed:** 2026-08-17
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments

- Added `NanoErrorKind::ModelLacksPdf` with the exact `model_lacks_pdf`, `-32602`, non-retryable ErrorResponse contract.
- Pinned the canonical and protocol-shim tables at 71 entries with source-level mapping assertions.
- Regenerated the in-repo and canonical sibling shared JSON mirrors through `gen_error_table`, with the Desktop target isolated to a unique absent temporary path.
- Made the canonical shared mirror fail-closed in check mode whenever the monorepo is detected, with a focused missing-target regression.
- Passed targeted source suites, generator check mode, `git diff --check`, and D9 ownership checks before and after the work.

## Task Commits

Commits are intentionally deferred to the parent integrator, which owns atomic commits on the authorized `feat/wp-03` worktree.

## Files Created/Modified

- `crates/nano-session/src/error_kind.rs` - Adds the closed error vocabulary variant.
- `crates/nano-session/src/error_codes.rs` - Adds the exact error specification, exhaustive membership, and pinned contract tests.
- `crates/nano-protocol/src/error_codes.rs` - Pins the re-exported table count and presentation.
- `crates/nano-cli/src/bin/gen_error_table.rs` - Requires the canonical shared mirror in monorepo check mode and distinguishes missing optional targets from unreadable targets.
- `crates/nano-session/contracts/nano-error-codes.json` - Generator-produced in-repo mirror.
- `D:/Development/waylandnano/shared/contracts/nano-error-codes.json` - Generator-produced canonical shared mirror.

## Decisions Made

- Followed the locked D2 refusal wording exactly and did not add routing behavior or unrelated error kinds.
- Used a unique nonexistent OS-temp Desktop directory for both generation and check mode; it remained absent.

## Deviations from Plan

### Review Fixes

**1. [HIGH - Fail-open generated mirror] Required the canonical shared target**
- **Found during:** Parent review after Task 2
- **Issue:** The generator marked the canonical shared target optional, so `--check` could pass when that mandatory mirror was missing.
- **Fix:** Marked the resolved shared target required, made missing required and all unreadable targets fail check mode, and retained NotFound skipping only for optional Desktop mirrors.
- **Files modified:** `crates/nano-cli/src/bin/gen_error_table.rs`
- **Verification:** Focused isolated missing-target test, real generator/check, mirror parity, and D9 ownership check.

**Total deviations:** 1 review fix.

Commit creation remains explicitly delegated to the parent integrator because it owns the authorized WP branch.

## Issues Encountered

- D9 full-tree hashing took about 16-17 minutes per pass; both required invocations completed successfully.
- The TDD RED gate failed as expected because `ModelLacksPdf` did not yet exist. After implementation, the only intermediate failure was the planned stale-artifact alarm, resolved by canonical generation.

## Known Stubs

None.

## User Setup Required

None.

## Next Phase Readiness

- The CLI pre-network PDF compatibility gate can now emit the canonical refusal after binding resolution.
- No ownership, generator parity, or Desktop-mirror blocker remains.

## Self-Check: PASSED

- All five declared implementation/artifact files exist.
- Both generator-owned mirrors contain `model_lacks_pdf` and are byte-identical.
- Targeted error tests, the focused generator regression, real generator check mode, `git diff --check`, and final D9 ownership check passed.

---
*Phase: 03-wp-0.3-pdf-intake*
*Completed: 2026-08-17*
