---
phase: 02-wp-0-2-memory-hardening
plan: 02
subsystem: memory-profile
tags: [measurement, pws, canary, decision]
requires: [02-01]
provides: [900-second-profile, measured-neither-proposal]
affects: [02-03, 02-04, 02-05]
tech-stack:
  added: []
  patterns: [audited-process-wrapper, pid-segmented-correlation, exact-value-canary]
key-files:
  created:
    - scripts/soak/evidence/run-20260816T163631293Z/WP-0.2-PROFILE-DECISION.md
    - scripts/soak/evidence/run-20260816T163631293Z/mem-stats.ndjson
  modified: [docs/FOLLOWUPS.md, scripts/canary/scan.mjs]
decisions:
  - "Propose measured neither: eligible fold auxiliaries are 28.094% and tool growth is zero."
metrics:
  duration: 901636ms
  completed: 2026-08-16
status: complete
---

# Phase 2 Plan 02: Memory profile decision summary

A completed 900-second, three-PID profile measured retained structures against independent PWS and proposed `neither` under the signed dominance rule.

## Results

- Corrected run: `scripts/soak/evidence/run-20260816T163631293Z`.
- 1,441 turns over 901,636 ms; 57 reporter rows and 15 aligned oracle samples.
- Exact-value canary: PASS, 12 files, 269,655 bytes, zero hits.
- Eligible fold share: 28.094%; tool share: 0%; proposed arm: `neither`.
- B1 failed end-ratio/slope and scaled B5 failed. No correction, budget edit, or one-hour receipt was started.
- The earlier `run-20260816T161856444Z` remains `aborted_unclassified` and selects no arm.

## Deviations from Plan

None in the corrected rerun. The harness's nonzero exit reflects its completed B1/B5 measurement verdict; wrapper cleanup was clean and the full manifest/evidence remained valid for classification.

## Decisions Made

Measured `neither` is the only eligible proposal because neither authorized suspect reached 60% of positive accounted growth. Owner confirmation remains the Plan 02-02 blocking checkpoint.

## Self-Check: PASSED

The manifest, reporter, oracle, wrapper audit, exact canary inventory/receipt, and decision artifact exist. Schema/cadence, PID alignment, cleanup, hashes, and zero-hit receipt were verified.
