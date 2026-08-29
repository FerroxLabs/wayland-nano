---
phase: 07-wp-4-gate-cards-and-dogfood
plan: 05
subsystem: testing
tags: [gate-registry, canonical-json, seeded-mutation, exhaustive-validation, atomic-evidence]
requires:
  - phase: 07-02
    provides: sealed install-payload reference and six mutants
  - phase: 07-03
    provides: sealed provision packet reference and six mutants
  - phase: 07-04
    provides: sealed config probes and six detached-worktree source mutants
provides:
  - Canonical schema-1 registry with confined artifacts, exact closure bodies, digests, tool pins, and pack mappings
  - Deterministic stable-LCG rotation runner requiring exhaustive proof before k=2 sampling
  - Three exact-HEAD atomic manifests covering 18 caught selected mutant executions
affects: [07-06, 07-07, verifier-dogfood, gate-card-audit]
tech-stack:
  added: []
  patterns: [canonical compact JSON, exhaustive-before-sampling, ownership-scoped cleanup, atomic bounded evidence]
key-files:
  created: [gates/tests/validate-seeded.cjs]
  modified: [gates/registry.json, gates/tests/gates-card-schema.test.cjs]
key-decisions:
  - "Keep run artifacts outside closure argv/digests and use only direct-or-pinned-interpreter invocation shapes."
  - "Require the complete named exhaustive battery on every seed invocation; seeded sampling supplements rather than replaces whole-pool proof."
  - "Run recorded rotations serially because generator --check rewrites shared sealed fixtures and concurrent runs can observe atomic-writer lock files."
patterns-established:
  - "Seed manifests bind exact HEAD, validator, registry, cards, scripts, closures, fixture seals, selections, observations, exhaustive inputs, and cleanup."
  - "Cleanup proofs assert only the run's owned roots/registrations while retaining before/after global inventory digests for audit."
requirements-completed: [CARD-01, CARD-02, CARD-03, CARD-04]
coverage:
  - id: D1
    description: Canonical three-pack registry resolves confined cards, scripts, artifacts, closure pins, and requirements
    requirement: CARD-01
    verification:
      - kind: integration
        ref: "gates/tests/gates-card-schema.test.cjs#t-registry-closure-digests"
        status: pass
      - kind: integration
        ref: "cargo test -p nano-verify registry -- --nocapture"
        status: pass
    human_judgment: false
  - id: D2
    description: All sealed production cards satisfy the closed inventory/tool/mutant/validation contract
    requirement: CARD-02
    verification:
      - kind: integration
        ref: "gates/tests/gates-card-schema.test.cjs#t-card-schema-valid"
        status: pass
    human_judgment: false
  - id: D3
    description: Three exact-HEAD rotations selected k=2 per pack and caught all 18 selected executions after exhaustive proof
    requirement: CARD-03
    verification:
      - kind: integration
        ref: "gates/tests/validate-seeded.cjs --seed 41041|41042|41043"
        status: pass
    human_judgment: false
  - id: D4
    description: References and all whole-pool mutants pass the complete named battery before every recorded sample
    requirement: CARD-04
    verification:
      - kind: integration
        ref: "three manifests: exhaustive.status=green and 13 required tests discovered exactly once"
        status: pass
    human_judgment: false
duration: 96min
completed: 2026-08-21
status: complete
---

# Phase 7 Plan 05: Canonical Registry and Seeded Validation Summary

**A canonical three-pack registry plus three exact-HEAD exhaustive-first rotations with 18 caught selected mutants and atomic cleanup-bound evidence**

## Performance

- **Duration:** 96 min
- **Completed:** 2026-08-21
- **Tasks:** 2
- **Plan-owned files modified:** 3

## Accomplishments

- Populated the schema-1 bootstrap through the governed atomic writer with canonical compact JSON, three exact closure digests, confined run artifacts, card-mirrored tool pins, and CARD-05/06/07 pack mappings.
- Added independent Node validation for the closed production cards and registry bytes, cross-checked by the landed Rust `nano-verify` loader/canonicalizer.
- Implemented a stable-LCG runner that rebuilds required binaries, requires the complete 13-name and whole-pool battery, selects exactly two mutants per pack, executes every selection, proves owned cleanup, and writes a bounded canonical manifest atomically.
- Recorded seeds 41041, 41042, and 41043 against exact HEAD `2b06602e94518ed501a7c4b4059f080e7ca1e2f6`; all 18 selected executions were caught.

## Task Commits

1. **Task 1 RED: canonical registry contract** - `334aacd`
2. **Task 1 GREEN: populated canonical registry** - `2036839`
3. **Task 2 RED: seeded validator entrypoint** - `03e3cfb`
4. **Task 2 GREEN: exhaustive-first seeded rotations** - `25a8c55`
5. **Task 2 cleanup concurrency fix** - `2b06602`

Consumed dependency repairs, retained as separate owner commits:

- `24aad7e` - canonical one-artifact install registry invocation.
- `a488651`, `9dc346d` - LF-stable config gate bytes and owned-root cleanup assertion.
- `fa0ffc8` - LF-stable provision gate bytes with fresh-checkout proof.

## Files Created/Modified

- `gates/registry.json` - Canonical three-pack schema-1 registry.
- `gates/tests/gates-card-schema.test.cjs` - Closed production-card and independently recomputed registry closure validation.
- `gates/tests/validate-seeded.cjs` - Exhaustive prerequisite, deterministic selection, real mutation execution, cleanup proof, and atomic evidence persistence.

## Recorded Rotation Evidence

| Seed | Install | Provision | Config | Manifest SHA-256 |
|---|---|---|---|---|
| 41041 | ip-m3, ip-m1 | pv-m1, pv-m3 | cf-m5, cf-m2 | `a00a2c6b2469bb4f8b9694b9c117b531d6632f60e7cb07782993f8320fc3705d` |
| 41042 | ip-m2, ip-m1 | pv-m2, pv-m4 | cf-m2, cf-m3 | `1e562c5afaa3c19cc0dffb319da6a678a6e9f6f7364ab991cd28accf43ff7389` |
| 41043 | ip-m1, ip-m6 | pv-m3, pv-m5 | cf-m5, cf-m3 | `2db2c796f2705157baab1e91ad7bb881e4dbf6a8fd7d46000aa743b05ff15e2e` |

The manifests are F-resident ephemeral execution evidence and are not committed. Each records `rotation_k: 2`, six caught observations, exact input digests, exhaustive green status, and absence of owned cleanup residue.

## Decisions Made

- Registry entries preserve the canonical WP3 contract: the verifier appends `run_artifact`; no artifact path is admitted into the hashed closure body.
- Node and Bash pins mirror the card machine blocks exactly, including the config gate's shipped `wayland-nano workspace` tool pin.
- Recorded rotations run serially. A parallel experiment correctly failed closed because config generator `--check` rewrites shared fixture files through per-file locks; serial execution preserves sealed-set consistency without weakening the prerequisite.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Reconciled canonical install invocation**
- **Found during:** Task 1 registry integration
- **Issue:** WP3 appends one artifact path, while the install gate accepted only the two-argument authoring form.
- **Fix:** Consumed the Plan 02 owner repair preserving sealed two-argument tests and adding trusted one-artifact registry mode.
- **Committed in:** `24aad7e`

**2. [Rule 1 - Bug] Pinned script bytes to LF on Windows**
- **Found during:** Task 2 exhaustive prerequisite
- **Issue:** Windows autocrlf changed config/provision working bytes and invalidated canonical card hashes.
- **Fix:** Consumed pack-owner `.gitattributes` repairs and fresh-checkout tests; canonical hashes now match on Windows and Linux.
- **Committed in:** `a488651`, `fa0ffc8`

**3. [Rule 1 - Bug] Scoped cleanup assertions to owned roots**
- **Found during:** Task 2 under the requested swarm
- **Issue:** Equality of the global Git worktree list falsely failed when unrelated agents changed sibling registrations.
- **Fix:** Require absence of the current control root/registration and retain before/after inventory digests as evidence.
- **Committed in:** `9dc346d`, `2b06602`

**4. [Rule 1 - Bug] Created validated evidence roots before atomic replacement**
- **Found during:** first complete seeded pass
- **Issue:** The atomic writer correctly failed when the validated destination directory did not yet exist.
- **Fix:** Create only the confined F: evidence root before the writer acquires its lock and performs same-directory atomic replacement.
- **Committed in:** `25a8c55`

**Total deviations:** four correctness/blocking fixes, all within the registry/pack integration contract. No producer, `.github`, merge, push, D:, dependency, or expansion scope was added.

## Verification

- Three recorded runs each completed the full 26-test Gate Card battery; all 13 required names were discovered exactly once and every exhaustive mutant test was green.
- Cross-manifest consistency: seeds distinct, `rotation_k=2`, 6 observations/run, 18/18 selected executions caught, exact HEAD/registry/validator/card/script/fixture bindings agree, cleanup flags true.
- `cargo test -p nano-verify registry -- --nocapture`: 3/3 registry unit tests pass.
- Focused Node registry/card command: 12/12 tests pass.
- No owned seeded worktree registration, control root, artifact lock, or tempfile remains.

## Known Stubs

None.

## Self-Check: PASSED

All plan artifacts and task/dependency commits exist. All three manifest hashes were recomputed from disk and all 18 observations independently checked against their declared `must_fail` sets.

## Next Phase Readiness

The registry and repeatability evidence are ready for WP3-only good/bad dogfood. `.github` promotion remains integrator-owned and untouched.

---
*Phase: 07-wp-4-gate-cards-and-dogfood*
*Completed: 2026-08-21*
