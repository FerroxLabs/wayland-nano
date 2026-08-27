---
phase: 01-ownership-contract-and-foundation
plan: 02
subsystem: testing
tags: [pmem, github-actions, receipts, powershell]
requires:
  - phase: p-mem-1
    provides: immutable PR head and seven-leg CI run
provides:
  - SHA-bound PR #8 pre-merge receipt
  - deterministic P-MEM acceptance producer and verifier
  - focused acceptance evidence for the immutable PR head
affects: [01-03, foundation-acceptance]
tech-stack:
  added: []
  patterns: [BOM-free JSON receipts, pre-command SHA pinning, exact argv manifests]
key-files:
  created:
    - scripts/phase1/Test-Pr8Receipt.ps1
    - scripts/phase1/Invoke-PmemAcceptance.ps1
    - scripts/phase1/Test-PmemAcceptanceReceipt.ps1
    - .planning/phases/01-ownership-contract-and-foundation/evidence/pr-8-premerge.json
    - .planning/phases/01-ownership-contract-and-foundation/evidence/pr-8-acceptance.json
  modified: []
key-decisions:
  - "Read CODEOWNERS from its operative repository-root location at the immutable PR head."
  - "Record absent human review as PENDING; never infer approval from green CI."
patterns-established:
  - "Evidence producers pin HEAD immediately before every executed acceptance command."
  - "Receipt verifiers reject command, check, ordering, or SHA substitution."
requirements-completed: []
coverage:
  - id: D1
    description: "PR #8 immutable metadata, exact seven green checks, and CODEOWNERS scope are receipt-bound."
    requirement: REQ-FOUND-01
    verification:
      - kind: integration
        ref: "powershell -NoProfile -File scripts/phase1/Test-Pr8Receipt.ps1 -ReceiptPath .planning/phases/01-ownership-contract-and-foundation/evidence/pr-8-premerge.json -ExpectedState OPEN -ExpectedHead f602ee7dfb663a3d50cf36dd43cb172d5828b5cb -ExpectedRunId 32954434975"
        status: pass
    human_judgment: false
  - id: D2
    description: "Focused P-MEM tests, fmt, and clippy pass on the exact PR head with contract metrics."
    requirement: REQ-FOUND-01
    verification:
      - kind: integration
        ref: "powershell -NoProfile -File scripts/phase1/Test-PmemAcceptanceReceipt.ps1 -ReceiptPath .planning/phases/01-ownership-contract-and-foundation/evidence/pr-8-acceptance.json -ExpectedSha f602ee7dfb663a3d50cf36dd43cb172d5828b5cb"
        status: pass
    human_judgment: false
duration: 18min
completed: 2026-08-27
status: complete
---

# Phase 1 Plan 02: PR #8 Acceptance Evidence Summary

**HISTORICAL/SUPERSEDED:** receipts prove the original P-MEM bars at `f602ee7`, but PR #10/CODEOWNERS changes moved PR #8 head. These artifacts cannot authorize final audit, signature, review, or merge; Plan 01-03 Task A regenerates canonical `head_sha`/`expected_run_id` receipts after protection.

## Performance

- **Duration:** 18 min
- **Started:** 2026-08-27T08:12:00Z
- **Completed:** 2026-08-27T08:29:58Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments

- Bound PR #8 to full SHA `f602ee7dfb663a3d50cf36dd43cb172d5828b5cb`, CI run `32954434975`, and exactly seven successful checks.
- Confirmed operative root `CODEOWNERS` rules for `/gates/**` and `/agents/**`; recorded review as `PENDING` without bypass.
- Ran the exact seven-command focused acceptance manifest against the untouched P-MEM worktree under `F:\CargoTarget\wayland-nano`.
- Proved recall@10 `1.0`, zero cross-project rows, zero cross-agent rows, kill/rebuild equivalence including `agent_id`, visible mediation receipt, and zero exit codes for focused tests, fmt, and clippy.

## Task Commits

Task commits were not created by this executor because the linked worktree is on `plan/persistent-agent-program`, while the mandatory executor guard permits agent commits only on `worktree-agent-*` branches. The parent orchestrator owns integration on the authorized planning branch.

## Files Created/Modified

- `scripts/phase1/Test-Pr8Receipt.ps1` - Captures and strictly verifies immutable GitHub and CODEOWNERS evidence.
- `scripts/phase1/Invoke-PmemAcceptance.ps1` - Runs the locked SHA-checked command manifest and emits a BOM-free receipt.
- `scripts/phase1/Test-PmemAcceptanceReceipt.ps1` - Independently rejects altered command sets, SHAs, failures, or deficient metrics.
- `.planning/phases/01-ownership-contract-and-foundation/evidence/pr-8-premerge.json` - PR/check/review/CODEOWNERS receipt.
- `.planning/phases/01-ownership-contract-and-foundation/evidence/pr-8-acceptance.json` - Exact local command outputs and acceptance metrics.

## Decisions Made

- Used the repository-root `CODEOWNERS`, the actual immutable-tree location, after isolating the plan context's incorrect `.github/CODEOWNERS` path.
- Kept REQ-FOUND-01 open because this plan proves pre-merge readiness but does not perform the required human merge or post-merge fresh-checkout verification.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Corrected CODEOWNERS source location**
- **Found during:** Task 1
- **Issue:** The context reference named `.github/CODEOWNERS`, but immutable tree inspection proved PR #8 ships `CODEOWNERS` at repository root.
- **Fix:** Read `contents/CODEOWNERS` at the pinned full SHA and kept the same operative-rule checks.
- **Files modified:** `scripts/phase1/Test-Pr8Receipt.ps1`
- **Verification:** Receipt verifier passed with both required scopes true.

**2. [Rule 3 - Blocking] Used PowerShell 5.1-compatible BOM-free UTF-8 output**
- **Found during:** Task 1
- **Issue:** Windows PowerShell 5.1 does not support `Set-Content -Encoding utf8NoBOM`.
- **Fix:** Used `[IO.File]::WriteAllText` with `[Text.UTF8Encoding]::new($false)`.
- **Files modified:** `scripts/phase1/Test-Pr8Receipt.ps1`, `scripts/phase1/Invoke-PmemAcceptance.ps1`
- **Verification:** Both JSON receipts parse and both strict verifiers pass.

**Total deviations:** 2 auto-fixed (1 plan path bug, 1 platform compatibility blocker)
**Impact on plan:** Both fixes preserve the locked evidence semantics; no implementation or scope expansion occurred.

## Issues Encountered

- The executor branch guard prevented child-agent commits on the authorized planning feature branch. No unsafe branch manipulation was attempted.

## User Setup Required

None. Human review and merge remain a later explicit checkpoint, not setup.

## Next Phase Readiness

- Do not use these f602 artifacts for final review. Bootstrap PR #10, install protection, and regenerate both PR #8 receipts first.
- REQ-FOUND-01 remains open until human merge and fresh-checkout post-merge verification.

## Self-Check: PASSED

- All five declared deliverable files exist.
- Both strict receipt verifiers pass.
- The P-MEM worktree remains clean and at the expected SHA.

---
*Phase: 01-ownership-contract-and-foundation*
*Completed: 2026-08-27*
