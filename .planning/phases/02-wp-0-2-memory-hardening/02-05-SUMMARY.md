---
phase: 02-wp-0-2-memory-hardening
plan: 05
subsystem: memory-handoff
tags: [audit, canary, handoff, measured-neither]
requires: [02-04]
provides: [wp-0.2-builder-handoff, immutable-evidence-inventory]
affects: [integrator-promotion, F-45]
tech-stack:
  added: []
  patterns: [exact-list-canary, stage-zero-evidence-verification]
key-files:
  created:
    - scripts/soak/evidence/WP-0.2-HANDOFF.md
  modified:
    - scripts/canary/scan.mjs
    - docs/FOLLOWUPS.md
decisions:
  - "Hand off the measured-neither result with F-45 open and no one-hour receipt or product correction."
  - "Resolve exact-list paths from the current worktree root while preserving the scanner's historical default root behavior."
metrics:
  inventory_files: 17
  inventory_bytes: 281189
  canary_hits: 0
  completed: 2026-08-17
status: complete
---

# Phase 2 Plan 05: WP-0.2 Builder Handoff Summary

WP-0.2 is locally gated and handed off as measured-`neither`, with F-45 still open, no product correction, and no one-hour acceptance receipt.

## Results

- Baseline: `566e3ac`; Plan 02-05 execution began from `bd56f8c`.
- Audit: the Critical/High pass and bounded final review found no Critical issues and three High defects, all resolved within the authorized fix rounds.
- Bounded fixes: exact-list inventory resolution, confinement, synthetic self-test paths, and governed-key ancestor search now use the active linked-worktree root; exact-list self-tests explicitly reject missing files, lexical traversal, and realpath junction/symlink escapes; the handoff now states the corrected indexed/cached evidence invariant. Legacy default scanning retains its historical root and behavior.
- Focused gates: scanner syntax/self-test, five memory-stat tests, two incremental-fold tests, and all 306 `nano-agent` unit tests passed; integration and doc tests also passed.
- Full gate: `just gate-all` passed, including formatting, workspace clippy/tests, generated error-table checking, and `gate-gen-check`. The first invocation was terminated by the command wrapper's 120-second timeout; the exact unchanged retry with sufficient time passed.
- Evidence: the two exact run directories plus handoff comprise 17 files and 281189 bytes. Exact-value canary scanning reported zero hits.
- Closure verification: the corrected Plan 02-05 verifier passed filesystem/inventory equality, receipt hash/byte equality, complete approved index equality, stage-0 worktree/index hash and size equality, cached-subset/change equality, ignore classification, and unchanged evidence `.gitignore`.
- Builder boundary: no merge, push, CI, promotion, budget edit, harness edit, product correction, or one-hour soak was performed.

## Outcome

The signed quantitative rule confirmed measured-`neither`. The prior short attempt remains classified as aborted/unclassified evidence. F-45 remains OPEN, and B1/B11 acceptance remains NOT EVALUATED because Plan 02-04 correctly took the ineligible/no-receipt branch.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Corrected exact-list scanner resolution in linked worktrees**

- **Found during:** Task 1 Critical/High audit.
- **Issue:** exact-list entries are repository-relative, but the historical scanner root resolves to the linked worktree's parent/main checkout, so exact inventory scanning could target the wrong checkout.
- **Fix:** introduced a current-worktree repository root for exact-list operations and tests while retaining the historical root for legacy default scanning.
- **Files modified:** `scripts/canary/scan.mjs`.
- **Commit:** uncommitted for root integrator review, as required by the branch allow-list.

**2. [Rule 2 - Missing critical validation] Proved exact-list confinement failure modes**

- **Found during:** bounded final review.
- **Issue:** the synthetic scanner test did not explicitly prove rejection of missing files, lexical traversal, or realpath escapes through junctions/symlinks.
- **Fix:** added fail-closed fixtures for all three cases; fixture creation errors fail the self-test.
- **Files modified:** `scripts/canary/scan.mjs`.
- **Commit:** uncommitted for root integrator review, as required by the branch allow-list.

**3. [Rule 1 - Bug] Corrected the handoff promotion invariant**

- **Found during:** bounded final review.
- **Issue:** the handoff incorrectly asked for the cached evidence diff to equal the entire inventory, including unchanged tracked evidence.
- **Fix:** require complete indexed-approved equality to scanned inventory, per-file stage-0/worktree hash-and-byte equality, and cached-approved equality only to the changed approved inventory subset.
- **Files modified:** `scripts/soak/evidence/WP-0.2-HANDOFF.md`.
- **Commit:** uncommitted for root integrator review, as required by the branch allow-list.

## Decisions Made

No fold or tool correction is promoted from this phase. The integrator receives an honest measured-neither handoff and must not treat the 900-second profile as a one-hour acceptance receipt.

## Known Stubs

None.

## Self-Check: PASSED

The handoff and both closure manifests exist; all 17 inventory files match the refreshed zero-hit receipt and stage-0 index; the corrected final automated verifier passed; the evidence roots and handoff received no writes after final inventory creation.
