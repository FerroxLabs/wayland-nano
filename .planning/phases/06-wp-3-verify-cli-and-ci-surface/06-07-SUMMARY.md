---
phase: 06-wp-3-verify-cli-and-ci-surface
plan: 07
subsystem: ci
tags: [github-actions, receipts, git-diff, actionlint, powershell]
requires:
  - phase: 05-wp-2-verified-change-engine
    provides: schema-1 receipt verification contract
provides:
  - Exact verify CLI operator contract and receipt honesty boundary
  - Schema-pinned docs-only receipt and future dogfood CI consumers
  - Hermetic real-Git A/M/D/R receipt-selection oracle
affects: [06-wp-3-verify-cli-and-ci-surface, 07-wp-4-sealed-mutants]
tech-stack:
  added: []
  patterns: [name-status receipt selection, docs-only workflow promotion]
key-files:
  created:
    - docs/verify/VERIFY-CLI.md
    - docs/verify/CI-ADOPTION.md
    - docs/verify/ci/verify-receipt-check.yml
    - docs/verify/ci/verify-dogfood.yml
    - docs/verify/ci/test-receipt-diff.ps1
  modified: []
key-decisions:
  - "Pin the schema-1 CI consumer to waylandnano 0.3.0 and reject receipt deletion or rename before invoking the verifier."
  - "Keep workflow promotion and required-check selection owner-only and blocked on WP-4 sealed mutants."
patterns-established:
  - "Receipt CI selection: real git diff --name-status drives A/M verification and D/R hard failure."
  - "Workflow ownership: author under docs/verify/ci and defer .github promotion to the owner."
requirements-completed: [CLI-06]
coverage:
  - id: D1
    description: "Exact operator CLI, event, exit, detached-rerun, and receipt honesty contract"
    requirement: CLI-06
    verification:
      - kind: other
        ref: "rg required-mode/exit/provenance/owner contract check"
        status: pass
    human_judgment: false
  - id: D2
    description: "Pinned CI selection rejects D/R and verifies A/M using real Git output"
    requirement: CLI-06
    verification:
      - kind: integration
        ref: "docs/verify/ci/test-receipt-diff.ps1"
        status: pass
      - kind: other
        ref: "actionlint docs/verify/ci/verify-receipt-check.yml docs/verify/ci/verify-dogfood.yml"
        status: pass
    human_judgment: false
duration: 14min
completed: 2026-08-20
status: complete
---

# Phase 6 Plan 07: Verify CI Adoption Summary

**Schema-pinned docs-only CI consumers with a real-Git oracle that proves A/M verification and D/R rejection without crossing the owner promotion boundary**

## Performance

- **Duration:** 14 min
- **Started:** 2026-08-20T19:24:00Z
- **Completed:** 2026-08-20T19:38:36Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments

- Documented all three verify modes, parser defaults/caps, the 0/1/2/3/6 exit matrix, closed JSONL frames, detached receipt reruns, and the identifiers-only/log-digest honesty boundary.
- Added pinned docs-only receipt and future WP-4 dogfood consumers with full-history checkout and minimal permissions.
- Added a hermetic F:-resident oracle that creates actual Git A/M/D/R histories, proves the selector outcomes, checks workflow arm parity, and always removes its repository.

## Task Commits

1. **Task 1: Document the exact CLI, events, verdicts, and honesty boundary** - `b0acbfa`
2. **Task 2: Author and validate the receipt and future dogfood consumers** - `85a0145`

## Files Created/Modified

- `docs/verify/VERIFY-CLI.md` - Exact argv, parsing, exit, event, detached-rerun, and receipt honesty contract.
- `docs/verify/CI-ADOPTION.md` - Post-WP-4 owner promotion and branch-protection procedure.
- `docs/verify/ci/verify-receipt-check.yml` - Pinned receipt consumer with name-status fail-closed selection.
- `docs/verify/ci/verify-dogfood.yml` - Dormant post-WP-4 mutation and fixture receipt consumer.
- `docs/verify/ci/test-receipt-diff.ps1` - Hermetic real-Git selector oracle.

## Decisions Made

- Used the authoritative exact verifier pin `waylandnano@0.3.0`; no floating installer or action version was introduced.
- Preserved docs-only workflow ownership and deferred both `.github` promotion and required status selection until WP-4 lands.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## Known Stubs

- `docs/verify/ci/verify-dogfood.yml:29` intentionally names `run-sealed-mutants.ps1`, which WP-4 owns and has not landed. The workflow is dormant under `docs/` and must not be promoted until that WP-4 dependency exists and passes its mutation battery.

## Verification

- `actionlint` passed for both workflow documents.
- `test-receipt-diff.ps1` passed: unchanged/A/M exit 0; D/R exit nonzero; workflow case arms match.
- Exact-pin/floating-version and forbidden `.github/**` diff checks passed.
- `just gate-all` passed with F:-resident `TEMP`, `TMP`, and `CARGO_TARGET_DIR`.

## User Setup Required

None. Owner promotion is intentionally deferred until after WP-4 and is not a current setup step.

## Next Phase Readiness

- WP-3 integration can consume this docs-only lane.
- WP-4 must land the sealed mutant runner before owner promotion of the dogfood workflow.

## Self-Check: PASSED

- All five planned deliverables exist.
- Both task commits exist.
- No `.github/**` file changed.

---
*Phase: 06-wp-3-verify-cli-and-ci-surface*
*Completed: 2026-08-20*
