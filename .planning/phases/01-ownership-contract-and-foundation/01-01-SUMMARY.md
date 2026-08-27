---
phase: 01-ownership-contract-and-foundation
plan: 01
subsystem: authority-contract
tags: [authority, identity, issuer, acp, memory-security]
requires: []
provides:
  - Hash-backed unsigned v1.0 authority amendment ready for human ratification
  - Reproducible four-source signature-state and byte-equality preflight
affects: [01-03-owner-ratification, phase-02-authenticated-activation]
tech-stack:
  added: []
  patterns: [immutable-source-manifest, explicit-supersession, human-signature-gate]
key-files:
  created:
    - D:/Development/waylandnano/shared/reviews/research-0.2/specs/WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md
    - .planning/phases/01-ownership-contract-and-foundation/evidence/source-ratification-preflight.md
  modified: []
key-decisions:
  - "Desktop exclusively owns product bot composition and scheduling; Nano owns authenticated security, continuity, bounded execution, and evidence."
  - "Wire principal_id remains a 1:1 semantic alias of immutable physical agent_id in v1.1."
  - "Ratification is a human gate: the prepared amendment is explicitly unsigned and cannot unlock Phase 2."
patterns-established:
  - "Source amendments enumerate immutable inputs, exact hashes, signature state, disposition, and precedence."
  - "Security enrollment is distinct from a product bot registry."
requirements-completed: []
coverage:
  - id: D1
    description: "Four authoritative source/snapshot pairs have matching SHA-256 values with explicit signature and disposition states."
    requirement: REQ-ARCH-01
    verification:
      - kind: other
        ref: "PowerShell Get-FileHash comparison in source-ratification-preflight.md"
        status: pass
    human_judgment: false
  - id: D2
    description: "Unsigned v1.0 amendment contains the complete owner-ratifiable authority, identity, issuer, carrier, fixture, merge-order, and compatibility contract."
    requirement: REQ-ARCH-01
    verification:
      - kind: other
        ref: "Mandatory-clause PowerShell content check from 01-01-PLAN.md"
        status: pass
    human_judgment: true
    rationale: "Only the human owner may judge and sign the amendment; automated completeness is not ratification."
duration: 6min
completed: 2026-08-27
status: complete
---

# Phase 1 Plan 01: Authority Amendment Preparation Summary

**Unsigned authority amendment with immutable source hashes, explicit Desktop/Nano ownership, issuer lifecycle, identity compatibility, and cross-repo promotion gates**

## Performance

- **Duration:** 6 min
- **Started:** 2026-08-27T08:24:48Z
- **Completed:** 2026-08-27T08:30:13Z
- **Tasks:** 2/2
- **Files created:** 2 deliverables plus this summary

## Accomplishments

- Recomputed all four authoritative source/snapshot SHA-256 pairs and proved equality without accessing `.secrets`.
- Prepared the exact unsigned v1.0 owner amendment with source precedence and explicit supersession instead of rewriting any governing snapshot.
- Pinned Desktop-exclusive product ownership, Nano kernel responsibilities, `principal_id`/`agent_id` compatibility, issuer lifecycle, `AcpConnection` carrier behavior, protected MEM-SEC ownership, exact-artifact merge order, and measurable compatibility exit criteria.

## Task Commits

Task commits were not created by this executor because the fixed shared planning worktree is on `plan/persistent-agent-program`, while the executor's mandatory worktree safety rule permits agent commits only on `worktree-agent-*` branches. The parent orchestrator must commit these already-verified artifacts from its authorized branch context.

## Files Created

- `D:/Development/waylandnano/shared/reviews/research-0.2/specs/WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md` — unsigned human-ratifiable amendment; SHA-256 `94FA973574DCD3E8738520A69E7897B3D2023CE2947EFAC102F5F62C210EBFC9`.
- `.planning/phases/01-ownership-contract-and-foundation/evidence/source-ratification-preflight.md` — reproducible source/signature/hash preflight; SHA-256 `D0494330847FF3AE575DA66D726E4119ED454CCBF47BC1C58894FBEAA6386B6F`.

## Decisions Made

- Followed locked D-01–D-10 exactly; no new architecture was introduced.
- Kept `REQ-ARCH-01` open because this plan prepares the artifact but human signature occurs in Plan 01-03.

## Deviations from Plan

None in deliverable scope. The first preflight shell expression had a PowerShell parser error; the required one-sentence hypothesis was recorded before attempt 2, and attempt 2 passed without changing inputs.

## Issues Encountered

- The shared amendment surface is not itself a Git repository, so its artifact cannot be included in a Nano task commit. Its exact path and digest are recorded for the parent/owner handoff.
- `scripts/phase1/` appeared as unrelated untracked work from another concurrent agent and was left untouched.

## Known Stubs

- The owner name, owner signature, and signature date fields are intentionally `PENDING`; Plan 01-03 is the required blocking human ratification checkpoint. This does not prevent Plan 01-01's preparation goal from being complete.

## Next Phase Readiness

- The amendment is ready for human review/signature in Plan 01-03 after the independent PR #8 evidence plan completes.
- Phase 2 remains blocked until owner signature and the remaining Phase 1 acceptance plans complete.

## Self-Check: PASSED

- Both plan-owned deliverables exist.
- All four source/snapshot SHA-256 pairs match.
- All mandatory amendment terms pass the plan's automated content check.
- Signature fields remain visibly pending; no owner approval is claimed.
- No implementation, Desktop, Phase 2, secret, merge, or tag work occurred.

---
*Phase: 01-ownership-contract-and-foundation*
*Completed: 2026-08-27*
