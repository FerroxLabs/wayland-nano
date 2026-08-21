---
phase: 07-wp-4-gate-cards-and-dogfood
plan: 01
subsystem: testing
tags: [node, gate-cards, sha256, atomic-write, fail-closed]
requires:
  - phase: 06-wp-3-verify-cli-ci
    provides: landed gate inventory and stdout parsing contracts
provides:
  - Closed Gate Card parser and canonical output/JSON helpers
  - Exact-byte canonical directory seals
  - Crash-safe cross-platform persistent artifact writer
  - Shared authoring and ownership boundary for all three WP-4 packs
affects: [07-02, 07-03, 07-04, gate-cards, dogfood]
tech-stack:
  added: []
  patterns: [Node-standard-library gate helpers, same-directory atomic replacement, sealed fixture directories]
key-files:
  created: [gates/lib/card.cjs, gates/lib/contract.cjs, gates/lib/dirhash.cjs, gates/lib/artifact-writer.cjs, gates/lib/atomic-replace-win32.ps1, gates/tests/gates-card-schema.test.cjs, gates/README.md]
  modified: []
key-decisions:
  - "Card authoring is a closed fail-closed schema while runtime stdout remains compatible with the landed WP-3 last-summary parser."
  - "Every persistent WP-4 byte uses one token-safe create-new lock and same-directory platform atomic replacement path."
patterns-established:
  - "Fixture seal: UTF-8-byte-sorted NFC paths plus exact-byte per-file SHA-256 manifest."
  - "Artifact persistence: create-new lock, synced tempfile, atomic replace, Unix directory fsync, unconditional cleanup."
requirements-completed: [CARD-02, CARD-04]
coverage:
  - id: D1
    description: Closed shared Gate Card, output, canonical JSON, and fixture-seal contracts
    requirement: CARD-02
    verification:
      - kind: unit
        ref: gates/tests/gates-card-schema.test.cjs#required named card and seal battery
        status: pass
    human_judgment: false
  - id: D2
    description: Crash-safe cross-platform artifact persistence and one-retry reads
    requirement: CARD-04
    verification:
      - kind: integration
        ref: gates/tests/gates-card-schema.test.cjs#artifact writer contention/failure/visibility/CLI tests
        status: pass
      - kind: integration
        ref: just gate-all
        status: pass
    human_judgment: false
duration: 18min
completed: 2026-08-21
status: complete
---

# Phase 7 Plan 01: Shared Gate Card Foundation Summary

**Closed Gate Card and byte-seal contracts with a tested crash-safe atomic writer for every WP-4 persistent artifact**

## Performance

- **Duration:** 18 min
- **Completed:** 2026-08-21
- **Tasks:** 2
- **Files modified:** 8

## Accomplishments

- Added dependency-free closed card parsing, canonical JSON, exact output grammar, mutant-defect scoring, and directory seals.
- Added token-safe contention/stale-lock handling, synced same-directory temporary writes, exact Windows `MoveFileExW` replacement, Unix rename plus parent fsync, and deterministic read recovery.
- Documented the three-pack authoring flow, producer ownership fence, F-only scratch rule, and WP-3-only dogfood entry point.

## Task Commits

1. **TDD RED: lock shared contracts** - `27ead4a`
2. **Task 1: shared card, seal, output, and writer foundation** - `d75c841`
3. **Task 2: operator authoring boundary** - `f0ffa05`

## Files Created/Modified

- `gates/lib/card.cjs` - Closed machine-block parser and validation-hash coherence.
- `gates/lib/contract.cjs` - Canonical JSON, output reconstruction, emission, and green-mutant defect scoring.
- `gates/lib/dirhash.cjs` - Exact-byte deterministic directory digest and seal CLI.
- `gates/lib/artifact-writer.cjs` - Import API and exact-byte read/write CLI with bounded locking and recovery.
- `gates/lib/atomic-replace-win32.ps1` - Governed exact `MoveFileExW(...MOVEFILE_REPLACE_EXISTING)` helper.
- `gates/fixtures/.gitattributes` - Disables checkout text conversion for sealed fixture bytes.
- `gates/tests/gates-card-schema.test.cjs` - Required named meta-tests and writer failure/concurrency battery.
- `gates/README.md` - Operational authoring, persistence, ownership, and dogfood contract.

## Decisions Made

- Kept shared helper tests minimally cardinality-neutral; pack-specific plans enforce six checks and at least five mutants per pack.
- Exposed deterministic structured results from seal/output helpers so later pack tests can independently recompute evidence.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

Plans 07-02 through 07-04 can consume the shared files without modifying them. Focused tests pass 10/10, the five exact required filtered names pass 5/5, and `just gate-all` passed with all build and temporary roots on F:.

## Self-Check: PASSED

All eight declared deliverable files and task commits `27ead4a`, `d75c841`, and `f0ffa05` exist.

---
*Phase: 07-wp-4-gate-cards-and-dogfood*
*Completed: 2026-08-21*
