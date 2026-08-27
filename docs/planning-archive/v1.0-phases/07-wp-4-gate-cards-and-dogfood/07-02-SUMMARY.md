---
phase: 07-wp-4-gate-cards-and-dogfood
plan: 02
subsystem: testing
tags: [node, npm-package, gate-card, sha256, mutation-testing]
requires:
  - phase: 07-wp-4-gate-cards-and-dogfood
    provides: closed card parser, exact directory seals, gate output contract, and atomic artifact writer
provides:
  - Six-check install-payload Gate Card and production Node gate
  - Sealed deterministic reference plus exactly six fluent-but-wrong install mutants
  - Exhaustive mutation, seal, fail-closed, cleanup, and producer-integrity tests
affects: [07-05, 07-06, 07-07, gate-registry, dogfood]
tech-stack:
  added: []
  patterns: [sealed copied-package inspection, one-fault mutation corpus, post-copy seal revalidation]
key-files:
  created: [gates/install-payload/card.md, gates/install-payload/gate.cjs, gates/install-payload/fixtures/generators/generators.cjs, gates/fixtures/install-payload, gates/tests/gates-install-payload.test.cjs]
  modified: []
key-decisions:
  - "Fixture intent records Unix executable modes inside the byte seal while the gate also checks checkout-time mode bits on Unix."
  - "The gate verifies the source seal and the copied snapshot seal before any semantic inspection, closing the copy race without touching producers."
patterns-established:
  - "Install corpus publication: private F: sibling staging, per-file atomic writer publication, stale-file removal, then card repin."
  - "Install scoring: closed six-check inventory with whole-platform primary/helper bijection and independent integrity rehashing."
requirements-completed: [CARD-03, CARD-04, CARD-05]
coverage:
  - id: D1
    description: Sealed install reference scores 6/6 and exactly six declared one-fault mutants drop their required checks
    requirement: CARD-03
    verification:
      - kind: integration
        ref: gates/tests/gates-install-payload.test.cjs#t-ip-reference-scores-mm and t-ip-mutants-caught
        status: pass
    human_judgment: false
  - id: D2
    description: Seal drift, missing subjects, malformed manifests, and copy races fail closed
    requirement: CARD-04
    verification:
      - kind: integration
        ref: gates/tests/gates-install-payload.test.cjs#install gate fails closed on seal drift missing subject and malfunction
        status: pass
    human_judgment: false
  - id: D3
    description: Actual npm payload semantics cover lifecycle, inventory, hashes, tamper refusal, wrapper, modes, and manifest schema without producer edits
    requirement: CARD-05
    verification:
      - kind: integration
        ref: gates/tests/gates-install-payload.test.cjs#install gate scores reference 6/6 and catches every sealed mutant
        status: pass
      - kind: other
        ref: git diff --exit-code -- packaging/npm
        status: pass
    human_judgment: false
duration: 43min
completed: 2026-08-21
status: complete
---

# Phase 7 Plan 02: Install Payload Gate Card Summary

**A sealed six-check npm install gate that scores the reference 6/6 and exhaustively catches six fluent payload mutants without changing packaging producers**

## Performance

- **Duration:** 43 min
- **Completed:** 2026-08-21
- **Tasks:** 2 TDD tasks
- **Files modified:** 107

## Accomplishments

- Generated and sealed a full five-platform npm package reference with primary/helper integrity metadata and exactly six documented one-fault mutants.
- Implemented copied-tree lifecycle, bidirectional inventory, independent size/SHA, tamper refusal, wrapper/version, Unix mode, and closed manifest-schema checks.
- Added exhaustive reference/mutant, deterministic regeneration, writer-routing, seal-drift, missing-subject, producer-integrity, and cleanup evidence.

## Task Commits

1. **TDD RED: lock install-payload behavior** - `6204a02`
2. **Task 1: generate and seal reference/mutant corpus** - `f60a125`
3. **Task 2: enforce actual-package install gate** - `d8ea76e`

## Files Created/Modified

- `gates/install-payload/card.md` - Closed six-check inventory, final script pin, reference seal, six mutant seals, rotation, gamed modes, and bans.
- `gates/install-payload/gate.cjs` - Fail-closed copied-package gate with pre/post-copy seal verification and unconditional scratch cleanup.
- `gates/install-payload/fixtures/generators/generators.cjs` - Deterministic generator, private staging publisher, inspection oracle, check mode, and atomic card repin.
- `gates/fixtures/install-payload/` - Sealed reference plus `ip-m1` through `ip-m6` complete package trees.
- `gates/tests/gates-install-payload.test.cjs` - Required named reference/mutation tests plus exhaustive production-gate and fail-closed coverage.

## Decisions Made

- Kept producer scripts and manifests byte-read-only; the generator derives fixture bytes and semantics from them but publishes only under `gates/**`.
- Revalidated the copied snapshot against the caller-selected seal before semantic reads so a source mutation during copy cannot become trusted input.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Sealed executable-mode intent**
- **Found during:** Task 1 fixture publication
- **Issue:** The canonical directory digest seals file bytes and paths but not Unix mode bits, so the stripped-helper mutant could otherwise share the reference seal.
- **Fix:** Added byte-sealed `.nano-fixture-modes.json` intent, published matching Unix modes, and made IP-05 require both intended and actual executable modes.
- **Files modified:** generator, gate, all fixture roots
- **Verification:** `t-ip-mutants-caught` requires `ip-m5` to drop IP-05; reference remains 6/6.
- **Committed in:** `d8ea76e`

**2. [Rule 2 - Missing Critical] Post-copy seal revalidation**
- **Found during:** Task 2 seal boundary review
- **Issue:** A source tree could change after its first digest but during recursive copy.
- **Fix:** Recompute and compare the selected seal over the private copied snapshot before semantic reads.
- **Files modified:** `gates/install-payload/gate.cjs`
- **Verification:** exhaustive reference/mutant run and seal-drift fail-closed test.
- **Committed in:** `d8ea76e`

**Total deviations:** 2 auto-fixed missing-critical issues. Both close required integrity boundaries; no scope expansion.

## Issues Encountered

- The first repository-wide gate attempt hit a pre-existing timing-sensitive `nano-verify` cancellation teardown test at 698 ms under concurrent builds. No verifier code or test was changed; the exact test passed in isolation and the complete cached `just gate-all` rerun passed with bounded test concurrency.

## User Setup Required

None - no dependency, credential, or service configuration was added.

## Next Phase Readiness

The registry and dogfood plans can consume the install card, gate, reference, and exact six-mutant pool. The final card hash is `05786d983e865ae8c104008759c51ec296d81abb30936ecc6bc4314f042b4655`; focused/exhaustive tests pass 5/5, `just gate-all` passes, and `git diff -- packaging/npm` is empty.

## Self-Check: PASSED

All declared files and commits `6204a02`, `f60a125`, and `d8ea76e` exist; the card validation pin matches the production gate and all seven fixture seals independently recompute.

---
*Phase: 07-wp-4-gate-cards-and-dogfood*
*Completed: 2026-08-21*
