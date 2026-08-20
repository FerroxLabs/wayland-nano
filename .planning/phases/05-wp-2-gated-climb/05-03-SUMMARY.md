---
phase: 05-wp-2-gated-climb
plan: 03
subsystem: testing
tags: [rust, compile-contract, privacy, cargo-offline, adversarial]
requires:
  - phase: 05-wp-2-gated-climb
    provides: deterministic climb types and sealed Plan 02 public APIs
provides:
  - independent downstream compile-contract coverage for WP-2 public opacity
  - positive source-compatibility coverage for the frozen parser, manifest, workspace, outcome, and gate APIs
affects: [05-04, 05-05, wp-3]
tech-stack:
  added: []
  patterns: [std-only temporary downstream crates, poison-tolerant full-lifetime serialization, isolated offline Cargo targets]
key-files:
  created: [crates/nano-verify/tests/wp2_public_contract.rs]
  modified: []
key-decisions:
  - "Keep all Plan 02 API references inside generated downstream source so the outer harness compiles independently after Plan 01."
  - "Reject dependency-resolution, manifest-parse, and unrelated Rust syntax failures before accepting a negative fixture diagnostic."
patterns-established:
  - "Each downstream crate receives its own OS-temp project and target directory while one static mutex covers creation, Cargo execution, and cleanup."
requirements-completed: [CLIMB-03, CLIMB-05]
coverage:
  - id: D1
    description: "Downstream callers cannot forge or mutate trusted WP-2 types, clone the workspace, call the private candidate constructor, use stale verdict fields, or smuggle root/manifest authority through the runner or outcome."
    requirement: CLIMB-03
    verification:
      - kind: integration
        ref: "crates/nano-verify/tests/wp2_public_contract.rs#downstream_cannot_forge_or_mutate_trusted_types and downstream_cannot_bypass_constructors_or_frozen_signatures"
        status: pass
    human_judgment: false
  - id: D2
    description: "The frozen public parser, derivation getters, workspace factory, outcome getters, candidate readback, and path-based compatibility runner remain source-usable downstream."
    requirement: CLIMB-05
    verification:
      - kind: integration
        ref: "crates/nano-verify/tests/wp2_public_contract.rs#supported_downstream_surface_compiles"
        status: pass
    human_judgment: false
duration: 12min
completed: 2026-08-20
status: complete
---

# Phase 05 Plan 03: Public Contract and Adversarial Harness Summary

**A std-only downstream Cargo harness now proves WP-2's supported API compiles while every trusted authority boundary remains opaque and unforgeable.**

## Performance

- **Duration:** 12 min
- **Started:** 2026-08-20
- **Completed:** 2026-08-20
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments

- Added isolated offline downstream crates covering literal construction, field mutation, workspace cloning, private constructor access, stale verdict forging, incorrect derivation inputs, runner authority smuggling, and outcome extraction attacks.
- Added a companion positive crate covering the intended parser, manifest, artifact, workspace, outcome, and landed path-based gate surface.
- Serialized every fixture's complete lifetime and gave each project a distinct F:-backed target during executor verification.

## Task Commits

1. **Task 1: Add the external source-compatibility and unforgeability harness** - `49f4b0c` (test)

## Files Created/Modified

- `crates/nano-verify/tests/wp2_public_contract.rs` - Generates isolated downstream crates and validates positive and negative public API contracts through offline Cargo checks.

## Decisions Made

- Kept future API references solely in generated source strings, preserving independent compilation after Plan 01.
- Required intended compiler privacy/type/arity evidence and explicitly rejected dependency, manifest, and syntax false positives.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Normalized Windows verbatim canonical paths for Cargo manifests**
- **Found during:** Task 1 RED execution
- **Issue:** `canonicalize()` returned a `\\?\` path that Cargo rejected as an invalid path URL, producing a false-positive manifest failure.
- **Fix:** Removed the Windows verbatim prefix before slash normalization in the generated path dependency.
- **Files modified:** `crates/nano-verify/tests/wp2_public_contract.rs`
- **Verification:** The full downstream matrix reached the intended compiler boundaries and passed 3/3 tests.
- **Committed in:** `49f4b0c`

---

**Total deviations:** 1 auto-fixed (1 Rule 1 bug)
**Impact on plan:** The fix was required for a valid Windows black-box oracle and did not expand scope.

## Issues Encountered

- The shared worktree branch is not in the executor's required `worktree-agent-*` namespace. Per the mandatory guard, the root integrator made the atomic task commit; this executor did not bypass the guard.
- The first Plan03 all-target clippy attempt reached a concurrent Plan02 `gate.rs` warning. The Plan02 owner resolved it; the definitive all-target clippy rerun passed with warnings denied.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Plan04/Plan05 can rely on an independent source-level opacity oracle in addition to Plan02's authoritative exact-test-33 probes.
- Plan03's focused downstream matrix and all-target clippy are green against the settled Plan02 public surface.

## Self-Check: PASSED

- `crates/nano-verify/tests/wp2_public_contract.rs` exists.
- Commit `49f4b0c` exists and contains the owned harness.
- `cargo test -p nano-verify --test wp2_public_contract -- --nocapture`: 3 passed, 0 failed against settled Plan02 commit `af3752a` (93.53 s).
- `cargo check -p nano-verify --tests`: passed after Plan02 APIs became visible.
- `cargo clippy -p nano-verify --all-targets -- -D warnings`: passed.

---
*Phase: 05-wp-2-gated-climb*
*Completed: 2026-08-20*
