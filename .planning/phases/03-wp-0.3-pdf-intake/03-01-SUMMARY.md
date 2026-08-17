---
phase: 03-wp-0.3-pdf-intake
plan: 01
subsystem: ownership-control
tags: [powershell, ownership, preflight, sha256, fail-closed]
requires: []
provides: [wp03_control_v1, durable-external-tree-baselines, exact-wp03-owns]
affects: [03-02, 03-03, 03-04, 03-05, 03-06, 03-07, 03-08, 03-09, 03-10, 03-11, 03-12, 03-13]
tech-stack:
  added: []
  patterns: [nul-delimited-git-parsing, bounded-runspace-hashing, exact-path-authorization]
key-files:
  created:
    - .planning/phases/03-wp-0.3-pdf-intake/03-OWNERSHIP-PREFLIGHT.ps1
    - .planning/phases/03-wp-0.3-pdf-intake/03-CONTROL.json
  modified: []
decisions:
  - Hard-linked external files are hashed as regular files; repository changed paths still reject every link or reparse ancestor.
metrics:
  tasks: 1
  completed: 2026-08-17
status: complete
---

# Phase 03 Plan 01: Ownership Preflight Summary

Installed and initialized the fail-closed D9 ownership verifier before any WP-0.3 product mutation, with exact plan-derived allowlists and durable full-tree SHA-256 baselines.

## What Was Built

- PowerShell 5.1-compatible `Initialize`, `Check`, and `Closure` modes.
- NUL-delimited parsing for complete tracked, untracked, copy, and rename path coverage.
- Exact repository ownership derived from all 13 plan frontmatters plus explicit phase artifacts.
- Every-ancestor repository path checks rejecting links, reparse points, missing paths, and worktree escapes.
- Full sorted `{path,type,sha256,bytes}` manifests for canonical external Nano, `resources/upstreams`, and shared trees.
- Fail-closed shared-delta and repository/shared pair validation.

## Verification

- `Initialize`: PASS.
- Control schema, baseline `d8702f22f76aac7dc2d7fcc77b34e4482557ee12`, branch `feat/wp-03`, 13 exact plan IDs, all plan/summary phase-artifact entries, and durable manifest hashes: PASS.
- Immediate `Check`: PASS after complete external-tree recomputation.
- `git diff --check`: PASS.
- Only the two declared control artifacts changed before task commit.

Durable authoritative manifests are referenced by absolute path and full SHA-256 in `03-CONTROL.json`. Inventory sizes at initialization were 563,038 Nano entries, 80,928 `resources/upstreams` entries, and 943 shared entries.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Corrected PowerShell NUL record splitting**
- **Found during:** Task 1 Initialize
- **Issue:** The first split overload retained an empty trailing record and failed closed.
- **Fix:** Used regex NUL splitting with explicit empty-record removal.
- **Files modified:** `03-OWNERSHIP-PREFLIGHT.ps1`
- **Commit:** `fccdc92`

**2. [Rule 3 - Blocking] Bounded full-tree hashing with runspaces**
- **Found during:** Task 1 Initialize
- **Issue:** Sequential hashing could not finish within the execution bound for the 489 GB external Nano inventory.
- **Fix:** Partitioned the complete regular-file set across eight bounded in-process runspaces, then combined and path-sorted the identical manifest record set. No path was excluded.
- **Files modified:** `03-OWNERSHIP-PREFLIGHT.ps1`
- **Commit:** `fccdc92`

## Operational Note

Timed-out shell calls left prior child `powershell.exe` Initialize processes running. One completed concurrently and replaced the control pointer with its own durable `50c0...` directory. Before commit, no matching process remained; all three manifests in that directory were byte-identical to the independently completed `8b7f...` set, and Check passed against the authoritative current control. The parent executor explicitly approved retaining the current control.

## Known Stubs

None.

## Self-Check: PASSED

- Both declared control files exist.
- Task commit `fccdc92` exists.
- Initialize and immediate Check passed against the durable authoritative manifests.
