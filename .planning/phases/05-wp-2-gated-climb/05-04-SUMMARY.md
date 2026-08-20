---
phase: 05-wp-2-gated-climb
plan: 04
subsystem: verification-contract
tags: [rust, public-api, provenance, mutation-testing, fail-closed]
requires:
  - phase: 05-wp-2-gated-climb
    provides: [strict climb core, trusted engine, downstream opacity harness]
provides:
  - frozen WP-2 root export closure
  - exact donor transformation ledger
  - 38 serial red-green mutation receipts bound to one pristine commit
affects: [05-05, wp-3]
tech-stack:
  added: []
  patterns: [single-mutant serial execution, byte-exact source restoration, short F-drive Windows test roots]
key-files:
  created: [.planning/phases/05-wp-2-gated-climb/05-MUTATION-RECEIPTS.json]
  modified: [UPSTREAM.md]
key-decisions:
  - "Mutation evidence is accepted only for behavior-specific RED diagnostics, never compiler, environment, fixture, or zero-test failures."
  - "Every final receipt is bound to pristine commit f63a165f77efee10a9d54b059779a8579e431e16 and mutations execute serially across the whole crate."
requirements-completed: [CLIMB-01, CLIMB-02, CLIMB-03, CLIMB-04, CLIMB-05]
duration: 95min
completed: 2026-08-20
status: complete
---

# Phase 5 Plan 04: Export, Provenance, and Mutation Closure Summary

**WP-2 now has a source-compatible opaque public facade, exact donor provenance, dependency-neutral closure, and 38 independently killed contract mutants with byte-exact restoration.**

## Performance

- **Duration:** 95 minutes
- **Completed:** 2026-08-20
- **Tasks:** 3
- **Final receipt count:** 38

## Accomplishments

- Confirmed the frozen root exports require no further wiring: the supported downstream surface compiles and all forbidden construction, mutation, cloning, path, root, and arity cases fail at their intended boundaries.
- Added exactly two WP-2 provenance rows (`climb.rs`, `engine.rs`) and amended only the existing `gate.rs` row.
- Proved manifests and `Cargo.lock` remain byte-unchanged from phase base `7bcbc12`, with cargo-deny policy green.
- Executed all 38 frozen ratchet, scheduler, budget, parser, manifest, gate, and driver operators one at a time. Every receipt records one selected test, a behavior-specific RED, exact source restoration, and an identical GREEN.

## Task Commits

1. **Task 1: Seal root export contract** - no code change required; final surface already matched frozen authority.
2. **Task 2: Record exact provenance closure** - `62d8bcd`
3. **Task 3: Record exact mutation receipts** - `108cc25`

Supporting prior-owner corrections discovered by the bounded mutation audit:

- `57a0417`, `c7aacdd` - strengthen and expose exact climb mutation teeth.
- `872cc55`, `3a1bd9e`, `5c245b2` - strengthen parser, manifest, gate, and driver mutation oracles.
- `a96d460` - correct base-tree digest binding to exact preimage length plus SHA-256.
- `f63a165` - make the deadline mutant fail through an explicit contract assertion.

## Files Created/Modified

- `UPSTREAM.md` - exact two-row addition and bounded gate-row amendment.
- `.planning/phases/05-wp-2-gated-climb/05-MUTATION-RECEIPTS.json` - deny-unknown 38-receipt ledger.

`crates/nano-verify/src/lib.rs` required no change; its existing exports passed the frozen positive and negative API matrix.

## Decisions Made

- Serialized all final mutations across the entire crate after detecting that parallel family runs contaminate whole-crate compilation, even with distinct source files and Cargo targets.
- Used semantic multi-edit mutants where the frozen operator necessarily spans more than one line, notably CRLF acceptance and detached generation futures.
- Used a short F:-only temporary root for downstream MSVC fixtures after proving longer nested paths caused `LNK1181` before Rust reached the intended contract boundary.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Corrected base-tree digest preimage binding**
- **Found during:** Task 3, M03
- **Issue:** The base digest bound raw preimage bytes plus their SHA-256; the frozen contract requires exact byte length plus SHA-256.
- **Fix:** Replaced raw-byte length binding with explicit length plus digest and added an independent exact digest oracle.
- **Files modified:** `crates/nano-verify/src/engine.rs`
- **Commit:** `a96d460`

**2. [Rule 2 - Missing Critical] Strengthened mutation-specific contract oracles**
- **Found during:** Task 3 preflight and live RED validation
- **Issue:** Several frozen tests initially failed for unrelated structure/fixture reasons or did not observe their named operator.
- **Fix:** Added behavior-specific assertions for all frozen families, including CRLF, preimage SHA, parser-call observability, cancellation/drop, prompt opacity, and deadline arithmetic.
- **Files modified:** `crates/nano-verify/src/climb.rs`, `crates/nano-verify/src/engine.rs`, `crates/nano-verify/src/gate.rs`
- **Commits:** `57a0417`, `c7aacdd`, `872cc55`, `3a1bd9e`, `5c245b2`, `f63a165`

**3. [Rule 3 - Blocking] Shortened the F-drive downstream fixture root**
- **Found during:** Final verification
- **Issue:** Deep nested F: fixture paths caused MSVC `LNK1181` before negative crates reached Rust privacy/type checks.
- **Fix:** Re-ran the identical contract surface using short F:-only temp root `F:\t\w4`; 3/3 passed.
- **Files modified:** None.

**Total deviations:** 3 auto-fixed (1 product correctness, 1 critical validation coverage, 1 environment-path blocker).

## Verification

- Exact Task 3 receipt verifier from `05-04-PLAN.md`: passed, 38/38.
- Downstream public contract: 3 passed, 0 failed.
- `cargo test -p nano-verify`: 44 unit + 7 gate-contract + 9 receipt-Git + 3 downstream passed; doc tests passed.
- `cargo clippy -p nano-verify --all-targets -- -D warnings`: passed.
- Manifest/lock diff from `7bcbc12fec0624aacbc3953e4f2c7d1a2c4414e0`: empty.
- `cargo deny check`: advisories, bans, licenses, and sources passed.
- `git diff --check`: passed.
- All temporary and Cargo target output remained on F:.

## Known Stubs

None.

## Next Phase Readiness

- Plan 05 can perform its bounded audit against a dependency-neutral, provenance-complete WP-2 surface with exact mutation evidence.
- WP-3 may consume the accepted artifact and parser without acquiring workspace construction, root, or mutable manifest authority.

## Self-Check: PASSED

- Both declared output files exist.
- Task commits `62d8bcd` and `108cc25` exist.
- The final receipt ledger contains exactly 38 unique IDs and passes the literal plan verifier.
- Live `climb.rs`, `engine.rs`, and `gate.rs` blobs equal the pristine blobs recorded by the final receipts.

---
*Phase: 05-wp-2-gated-climb*
*Completed: 2026-08-20*
