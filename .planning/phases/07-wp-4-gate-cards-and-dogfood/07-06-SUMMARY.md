---
phase: 07-wp-4-gate-cards-and-dogfood
plan: 06
subsystem: verification
tags: [gate-cards, dogfood, provenance, evidence]
requires: [07-05]
provides: [wp3-only-dogfood, promotion-contract, provenance-ledger]
affects: [07-07, 07-08, 07-09]
tech-stack:
  added: []
  patterns: [closed-evidence-schema, identifiers-only-results, atomic-artifact-write]
key-files:
  created:
    - docs/verify/gates.md
    - gates/tests/validate-evidence.cjs
    - .planning/phases/07-wp-4-gate-cards-and-dogfood/07-DOGFOOD-EVIDENCE.json
  modified: [UPSTREAM.md]
decisions:
  - Dogfood evidence is accepted only from the landed WP-3 run-only surface.
  - Workflow promotion remains an integrator-owned sibling job on Windows.
metrics:
  tasks: 2
  completed: 2026-08-21
status: complete
---

# Phase 07 Plan 06: Gate-card Dogfood and Promotion Summary

All three registered gates accept their good artifacts through `wayland-nano verify --gate <id> --run-only`; the prescribed `ip-m1`, `pv-m2`, and `cf-m3` arms independently return Red through the same production surface.

## Accomplishments

- Persisted a closed atomic ledger binding base/product identity, registry digest, invocation, seals, identifiers-only results, exit classes, and cleanup facts.
- Documented the exact integrator-owned `gate-cards` job, staging prerequisite, merge blockers, branch-protection handoff, and builder ownership stop.
- Added exact WP-4 adapted-file provenance and a fail-closed validator covering dogfood, provenance, workflow, and downstream evidence stages.
- Proved the retained dogfood evidence, operator documentation, and provenance ledger contain zero governed-key hits.

## Verification

- `node gates/tests/validate-evidence.cjs dogfood .../07-DOGFOOD-EVIDENCE.json` — valid.
- `node gates/tests/validate-evidence.cjs provenance UPSTREAM.md` — valid.
- Complete Node gate battery with explicit F-resident provision binaries — 28/28 passed.
- Exact-list canary — 3 files scanned, 0 hits; receipt and include list deleted.
- Producer/CI/status ownership diff against the integrated base — empty.
- `just gate-all` — formatting, strict clippy, workspace tests, error-table check, and contract check passed.

## Deviations from Plan

### Auto-fixed Issues

None in Plan 07-06. Execution consumed the already-integrated config-path, authentic npm staging, and WP-3 bootstrap-fixture repairs before final validation.

## Known Stubs

None.

## Self-Check: PASSED

All declared artifacts exist, task commits are present in history, the final worktree contains only this summary change, and task-owned F-resident scratch has been removed.
