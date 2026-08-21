---
phase: 07-wp-4-gate-cards-and-dogfood
plan: 04
subsystem: testing
tags: [gate-cards, config-schema, mutation-testing, detached-worktrees, bash]
requires:
  - phase: 07-01
    provides: shared card parser, directory seals, output contract, and atomic artifact writer
provides:
  - Config-schema Gate Card with a sealed nine-probe corpus and six exact source mutants
  - Black-box rules CLI and named provider-catalog pin verification scoring 6/6
  - Detached F-only mutation harness with success and injected-failure cleanup proof
affects: [07-06, 07-07, gate-registry, verifier-dogfood]
tech-stack:
  added: []
  patterns: [atomic deterministic generation, exact-producer source patches, detached worktree mutation]
key-files:
  created: [gates/config-schema/card.md, gates/config-schema/gate.sh, gates/config-schema/fixtures/generators/generators.cjs, gates/fixtures/config-schema/manifest.json]
  modified: [gates/tests/gates-config-schema.test.cjs]
key-decisions:
  - "Bind source mutants to producer-owner repair 30dbe9d while retaining locked D-04 base 05637086 in the manifest."
  - "Keep the shipped wayland-nano rules command as parser authority and use named parser/catalog anchors only as strict drift pins."
patterns-established:
  - "Every source mutant gets a detached exact-base worktree and private short F: Cargo target removed in finally cleanup."
  - "Generator output uses the shared atomic artifact writer and every patch header is confined to a closed producer-file allowlist."
requirements-completed: [CARD-03, CARD-04, CARD-07]
coverage:
  - id: D1
    description: Config reference scores 6/6 through the shipped rules CLI and named catalog pin
    requirement: CARD-04
    verification:
      - kind: integration
        ref: "gates/tests/gates-config-schema.test.cjs#t-cf-reference-scores-mm"
        status: pass
    human_judgment: false
  - id: D2
    description: Six sealed fluent-but-wrong source mutants are caught
    requirement: CARD-03
    verification:
      - kind: integration
        ref: "gates/tests/gates-config-schema.test.cjs#t-cf-mutants-caught"
        status: pass
    human_judgment: false
  - id: D3
    description: Detached mutation worktrees and private targets leave no residue after success or injected failure
    requirement: CARD-07
    verification:
      - kind: integration
        ref: "gates/tests/gates-config-schema.test.cjs#t-cf-cleanup-survives-injected-failure"
        status: pass
    human_judgment: false
duration: 68min
completed: 2026-08-21
status: complete
---

# Phase 7 Plan 04: Config Schema Gate Pack Summary

**A sealed 6/6 rules-and-catalog gate with six exact-source mutants and residue-free detached F: worktree execution**

## Performance

- **Duration:** 68 min
- **Completed:** 2026-08-21
- **Tasks:** 2
- **Files modified:** 20

## Accomplishments

- Generated and sealed the valid plus eight-invalid rules probe corpus, six committed unified-diff mutants, parser anchors, and named catalog authority through the atomic artifact writer.
- Proved the unchanged production gate scores the repaired reference 6/6 and catches cf-m1 through cf-m6 through the real built CLI.
- Proved each mutation uses a detached F-only worktree/private Cargo target and removes both paths and Git registrations after normal and injected-failure paths.

## Task Commits

1. **TDD RED: required config reference and mutation behavior** - `4090d39`
2. **Producer-owner repair: evaluator limits at stored-rule validation** - `30dbe9d`
3. **Task 1: sealed probes, mutants, card, generator, and black-box gate** - `a3b574e`
4. **Task 2: injected-failure worktree and target cleanup proof** - `298e327`

## Files Created/Modified

- `gates/config-schema/card.md` - Six-check inventory, sealed mutant declarations, rotation, bans, and validation pins.
- `gates/config-schema/gate.sh` - Strict black-box CLI/catalog gate with explicit fail-closed routing.
- `gates/config-schema/fixtures/generators/generators.cjs` - Deterministic atomic probe, patch, anchor, and manifest generator.
- `gates/fixtures/config-schema/` - Sealed probes, exact patches, and producer-anchor manifest.
- `gates/tests/gates-config-schema.test.cjs` - Reference scoring, exhaustive mutants, patch confinement, and cleanup tests.

## Decisions Made

- The owner-authorized producer repair is the mutation base because it contains the actual parser behavior under verification; the original locked base remains recorded separately for audit traceability.
- Windows probe ACLs are set to current-user full control before invocation so the shipped owner-only DACL audit passes while cleanup retains delete permission.
- `NANO_GATE_ROOT` separates immutable Gate Card assets from each detached producer worktree, allowing unchanged gate bytes to score every mutant.

## Deviations from Plan

### Owner-authorized repair

**1. CF-05 producer behavior was absent at locked D-04 base**
- **Found during:** Task 1 reference RED run
- **Issue:** evaluator limits existed but stored `PrefixRule` validation accepted over-limit patterns, making the required black-box reference score 5/6.
- **Resolution:** the producer owner supplied commit `f32163e`, cherry-picked as `30dbe9d`; this plan made no further producer edits.
- **Verification:** real `wayland-nano rules` rejection probes pass, producer unit/CLI tests pass, and `just gate-all` passes.

### Auto-fixed Issues

**1. [Rule 1 - Bug] Made the decision downgrade mutant compile**
- **Found during:** exhaustive cf-m6 build
- **Issue:** serde requires `#[serde(other)]` on the final enum variant.
- **Fix:** reordered the mutant-only enum variants with Prompt last; serialized names and production bytes remain unchanged.
- **Files modified:** `gates/config-schema/fixtures/generators/generators.cjs`, cf-m6 patch/seal, card seal.
- **Verification:** exhaustive mutant test passes all six.
- **Committed in:** `a3b574e`

**2. [Rule 1 - Bug] Preserved Windows cleanup after owner-only ACL enforcement**
- **Found during:** first reference gate run
- **Issue:** read/write-only ACL removed the delete right and made cleanup fail closed.
- **Fix:** grant only the current user full control on the ephemeral rules file; no beyond-user principal is admitted.
- **Verification:** 6/6 reference and injected cleanup tests pass.
- **Committed in:** `a3b574e`

**Total deviations:** one owner-authorized producer repair and two inline correctness fixes. No producer, catalog, registry, CI, merge, or push scope was added by this plan.

## Verification

- `node --test gates/tests/gates-config-schema.test.cjs`: 3/3 pass; six mutants caught.
- `just gate-all`: formatting, strict workspace clippy, workspace tests, and generated-contract checks pass.
- `git worktree list --porcelain` and `F:/w4m` inventory: no mutation worktree, target, or registration residue.

## Known Stubs

None.

## Self-Check: PASSED

All declared artifacts and commits exist; producer sources were not edited after the owner repair.

## Next Phase Readiness

The config-schema pack is ready for registry population and WP-3 dogfood integration. No Plan 07-04 blocker remains.

---
*Phase: 07-wp-4-gate-cards-and-dogfood*
*Completed: 2026-08-21*
