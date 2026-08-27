---
phase: 05-wp-2-gated-climb
plan: 02
subsystem: verification-engine
tags: [rust, async, fail-closed, unified-diff, sha256, subprocess]
requires:
  - phase: 05-wp-2-gated-climb
    provides: [pure strict-ratchet scheduler and sealed climb outcome]
provides:
  - sealed OS-temp candidate artifacts and complete-output gate evidence
  - sole schema-1 candidate-diff parser and immutable expected-change derivation
  - injected-effects async climb driver with checked deadlines and cancellation
affects: [05-03, 05-04, wp-3-materializer]
tech-stack:
  added: []
  patterns: [opaque core-owned filesystem authority, complete-output evidence, checked monotonic deadlines]
key-files:
  created: [crates/nano-verify/src/engine.rs]
  modified: [crates/nano-verify/src/gate.rs, crates/nano-verify/src/error.rs, crates/nano-verify/src/lib.rs]
key-decisions:
  - "Candidate bytes must pass the sole parser before core persists or gates them."
  - "Evidence is eligible only after complete bounded stdout capture and normal exit."
  - "Exact test 33 executes the downstream Cargo privacy/arity matrix itself."
patterns-established:
  - "Effects exposes generation, monotonic time, cancellation, and closed events only."
  - "Artifact handles retain private workspace lifetime and revalidate path plus digest on readback."
requirements-completed: [CLIMB-01, CLIMB-03, CLIMB-04, CLIMB-05]
coverage:
  - id: D1
    description: "Trusted artifact and complete-evidence gate execution APIs"
    requirement: CLIMB-03
    verification:
      - kind: integration
        ref: "gate::tests::wp2_gate_execution_evidence_matrix and gate::tests::wp2_workspace_candidate_confinement_matrix"
        status: pass
    human_judgment: false
  - id: D2
    description: "Strict candidate parser and read-only expected-change manifest"
    requirement: CLIMB-05
    verification:
      - kind: unit
        ref: "engine::tests::wp2_candidate_parser_matrix and engine::tests::wp2_expected_change_manifest_matrix"
        status: pass
    human_judgment: false
  - id: D3
    description: "Closed-effects asynchronous gated-climb driver"
    requirement: CLIMB-01
    verification:
      - kind: integration
        ref: "engine::tests::driver_stub_suite plus sixteen focused exact helpers"
        status: pass
    human_judgment: false
duration: 42min
completed: 2026-08-20
status: complete
---

# Phase 5 Plan 02: Trusted Gated-Climb Engine Summary

**A fail-closed async climb driver now parses generated diffs before persistence, gates sealed private artifacts, and returns only complete core-derived evidence.**

## Performance

- **Duration:** 42 minutes
- **Completed:** 2026-08-20
- **Tasks:** 3
- **Files modified:** 4

## Accomplishments

- Added opaque non-Clone workspaces, digest-bound candidate handles, baseline/candidate execution evidence, and detailed fail-closed outcomes while retaining WP1 `run_gate` behavior.
- Implemented the sole fully consuming schema-1 unified-diff parser and an in-memory, no-mutation expected-change derivation path.
- Implemented sequential generation, engine-owned gate execution, checked monotonic deadlines, bounded cancellation polling, sanitized provider failures, closed events, and terminal mapping.
- Proved all seventeen exact Plan02 names individually; the umbrella runs the external downstream privacy and arity matrix in its own isolated F-drive Cargo target.

## Task Commits

1. **RED registration:** `c271efb`
2. **Strict parser and manifest derivation:** `1a8413f`
3. **Sealed gate APIs and async driver:** `af3752a`
4. **Authority-owned downstream contract probe:** `273f139`

Supporting Plan01 seam corrections were committed by its owner as `1c4325b` and `4a6ea23`.

## Files Created/Modified

- `crates/nano-verify/src/engine.rs` - parser, manifest derivation, Effects seam, driver, and exact test-33 matrix.
- `crates/nano-verify/src/gate.rs` - opaque workspace/artifact types and complete-evidence execution APIs.
- `crates/nano-verify/src/error.rs` - artifact I/O conversion at the parser/filesystem boundary.
- `crates/nano-verify/src/lib.rs` - frozen public exports.

## Decisions Made

- Kept the original path-based WP1 runner intact and added evidence-bearing execution as a separate sealed surface.
- Used exact checked millisecond arithmetic; the local Clippy allowance documents why saturating subtraction is forbidden by authority.
- Nested the committed public-contract integration harness from exact test 33 so Plan03 supplements rather than substitutes for authority-owned coverage.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Added a crate-private sealed outcome constructor**
- **Found during:** Task 3
- **Issue:** Plan01 left no legal sibling-module path for the driver to construct `ClimbOutcome`.
- **Fix:** Plan01 owner added the sole crate-private constructor and a focused test.
- **Files modified:** `crates/nano-verify/src/climb.rs`
- **Verification:** full nano-verify suite and exact driver tests passed.
- **Committed in:** `1c4325b`

**2. [Rule 1 - Bug] Made a green final-budget call stop as Solved**
- **Found during:** Task 3 green probe
- **Issue:** budget was checked before the accepted green candidate, yielding NeedsEscalation.
- **Fix:** Plan01 owner restored solved-before-budget precedence.
- **Files modified:** `crates/nano-verify/src/climb.rs`
- **Verification:** `driver_green_probe_short_circuits` passed with Verified, one round, and readable accepted bytes.
- **Committed in:** `4a6ea23`

**Total deviations:** 2 correctness fixes, both confined to the missing Plan01/Plan02 seam.

## Verification

- Exact manifest: 17/17 names discovered exactly once and passed individually.
- `driver_stub_suite`: passed, including its isolated external Cargo contract matrix.
- `cargo test -p nano-verify`: passed.
- `cargo clippy -p nano-verify --all-targets -- -D warnings`: passed.
- Existing WP1 gate contract tests: passed.

## Known Stubs

None.

## User Setup Required

None.

## Next Phase Readiness

The public opacity harness is green against the settled APIs. Plan04 may now add provenance/generator integration and phase-wide promotion evidence; WP3 can consume the accepted artifact and sole parser without acquiring candidate-workspace authority.

## Self-Check: PASSED

- All declared source files exist.
- All four Plan02 commits exist.
- Required exact tests and downstream probes passed from the settled branch.

---
*Phase: 05-wp-2-gated-climb*
*Completed: 2026-08-20*
