---
phase: 07-wp-4-gate-cards-and-dogfood
plan: 03
subsystem: testing
tags: [node, provision, windows, mutation-testing, sealed-fixtures]
requires:
  - phase: 07-wp-4-gate-cards-and-dogfood
    provides: shared Gate Card, contract, directory seal, and atomic writer helpers
provides:
  - Producer-authentic deterministic provisioning packet corpus
  - Six-check portable packet gate and explicit Windows live no-mutation arm
  - Six sealed one-fault mutants with exhaustive detection
affects: [07-05, 07-06, 07-07, gate-cards, dogfood]
tech-stack:
  added: []
  patterns: [marker-framed producer capture, normalized deterministic fixtures, external-state equality oracle]
key-files:
  created: [gates/provision-script/card.md, gates/provision-script/gate.cjs, gates/provision-script/fixtures/generators/generators.cjs, gates/fixtures/provision-script/manifest.json, gates/tests/gates-provision-script.test.cjs]
  modified: []
key-decisions:
  - "Portable packet and Windows live modes are mutually exclusive and malformed selection fails closed at 0/6."
  - "The live arm runs setup non-elevated and accepts only refusal with an identical user/firewall/marker digest."
patterns-established:
  - "Provision corpus: invoke the real dry-run binary, extract only its marked JSON frame, normalize four machine-varying fields, then atomically persist exact bytes."
requirements-completed: [CARD-03, CARD-04, CARD-06]
coverage:
  - id: D1
    description: Real provisioning reference scores 6/6 and reproduces byte-for-byte
    requirement: CARD-06
    verification:
      - kind: integration
        ref: gates/tests/gates-provision-script.test.cjs#t-pv-reference-scores-mm
        status: pass
    human_judgment: false
  - id: D2
    description: All six sealed provisioning mutants drop their declared check
    requirement: CARD-03
    verification:
      - kind: mutation
        ref: gates/tests/gates-provision-script.test.cjs#t-pv-mutants-caught
        status: pass
    human_judgment: false
  - id: D3
    description: Windows setup refuses non-elevated execution without user firewall or marker mutation
    requirement: CARD-04
    verification:
      - kind: integration
        ref: gates/tests/gates-provision-script.test.cjs#Windows-live-arm
        status: pass
    human_judgment: false
duration: 31min
completed: 2026-08-21
status: complete
---

# Phase 7 Plan 03: Provision Script Gate Pack Summary

**Producer-authentic provisioning packets with six sealed mutants, a 6/6 portable gate, and a live Windows refusal/no-mutation oracle**

## Performance

- **Duration:** 31 min
- **Completed:** 2026-08-21
- **Tasks:** 2
- **Files modified:** 13

## Accomplishments

- Captured the real marker-framed `wayland-nano-provision-dry-run` JSON and normalized only machine-specific home, cwd, user, and cancellation-token values.
- Sealed a deterministic reference plus `pv-m1` through `pv-m6`, each with one fluent fault and declared expected drop.
- Implemented exact structure, identity, idempotence, confinement/no-mutation, version, and uninstall ownership checks.
- Proved the Windows live helper refuses non-elevated execution while the exact NanoSandbox user, firewall, and marker digest remains unchanged.

## Task Commits

1. **TDD RED: provision packet behavior** - `952e03e`
2. **Task 1: authentic sealed provision corpus** - `aebec87`
3. **Task 2: packet and Windows no-mutation arms** - `7d32b9b`
4. **Seal and rotation assertions** - `5d37a60`

## Verification

- Required focused reference/mutant tests: pass in three consecutive runs.
- Complete provision pack: 4/4 pass, including the Windows live oracle.
- Complete Node gate battery: 14/14 pass.
- `just gate-all`: pass with Cargo target and temporary roots on F:.
- Provisioning producer diff: empty.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Moved test TEMP outside the repository**
- **Found during:** Overall verification
- **Issue:** An F:-resident TEMP nested under the checkout made an unrelated ancestor-discovery test see repository `AGENTS.md`.
- **Fix:** Re-ran the complete gate with `F:/wp4p3-temp`, outside the checkout but still on F:.
- **Files modified:** None
- **Commit:** Not applicable

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: external-state-oracle | gates/provision-script/gate.cjs | Reads NanoSandbox users, firewall rules, and marker existence and invokes only the non-elevated setup refusal probe. |

## Known Stubs

None.

## Self-Check: PASSED

All declared files and commits exist; the worktree is clean after this summary commit.

---
*Phase: 07-wp-4-gate-cards-and-dogfood*
*Completed: 2026-08-21*
