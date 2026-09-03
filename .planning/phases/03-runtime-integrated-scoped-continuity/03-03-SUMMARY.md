---
phase: 03-runtime-integrated-scoped-continuity
plan: 03
subsystem: runtime-memory
tags: [sqlite, journal, acp, continuity, mediation]
requires:
  - phase: 03-01
    provides: scoped nano-memory store and mem-sec primitives
  - phase: 03-02
    provides: resolved MemoryPolicy, configured-agent set, and admitted continuity fallback
provides:
  - one authenticated scoped-memory seam across ACP, protocol-host, and exec
  - attributed backward-readable MemoryPolicyResolved audit records
  - dedicated-memory-journal topology decision and explicit rebuild policy authority
affects: [03-04, 03-05, 03-06]
tech-stack:
  added: []
  patterns: [session-coordinator policy audit, dedicated mutation journal, fresh per-prompt recall]
key-files:
  created:
    - .planning/phases/03-runtime-integrated-scoped-continuity/03-03-DECISION.md
    - crates/nano-cli/src/memory_seam.rs
    - crates/nano-cli/tests/activation_memory_seam.rs
  modified:
    - crates/nano-cli/src/acp_mode.rs
    - crates/nano-cli/src/host_mode.rs
    - crates/nano-cli/src/exec_run.rs
    - crates/nano-session/src/op.rs
    - crates/nano-memory/src/store.rs
    - crates/nano-memory/tests/corrective_regressions.rs
    - crates/nano-cli/tests/activation_quarantine.rs
key-decisions:
  - "Memory mutations use the dedicated memory journal; session JournalCoordinators own one attributed policy audit per persistent start."
  - "Caller-supplied resolved policy is the sole rebuild authority; session policy audit records are replay-neutral."
patterns-established:
  - "Authenticated memory identity is passed byte-for-byte from admitted project_id/principal_id."
  - "Model memory writes route only through commit_proposal; legacy memory tool names remain UnknownTool."
requirements-completed: [REQ-MEM-02]
coverage:
  - id: D1
    description: One scoped memory seam serves ACP new/load, protocol-host, and exec fresh/resume.
    requirement: REQ-MEM-02
    verification:
      - kind: integration
        ref: cargo test -p nano-cli --test activation_memory_seam --test activation_quarantine --test activation_admission -- --test-threads=1
        status: pass
    human_judgment: false
  - id: D2
    description: Attributed policy records remain backward-readable and grant no rebuild authority.
    requirement: REQ-MEM-02
    verification:
      - kind: integration
        ref: crates/nano-cli/tests/activation_memory_seam.rs#attributed_policy_record_round_trips_and_legacy_shape_stays_readable
        status: pass
      - kind: integration
        ref: cargo test -p nano-memory --test durability --test corrective_regressions -- --test-threads=1
        status: pass
    human_judgment: false
  - id: D3
    description: Model proposals are mediated and legacy or unconfigured-agent paths fail closed.
    requirement: REQ-MEM-02
    verification:
      - kind: integration
        ref: crates/nano-cli/tests/activation_memory_seam.rs#scoped_executor_mediates_proposals_and_denies_legacy_names
        status: pass
      - kind: integration
        ref: crates/nano-cli/tests/activation_memory_seam.rs#seam_open_refuses_an_unconfigured_agent
        status: pass
    human_judgment: false
duration: 1h 20m
completed: 2026-09-03
status: complete
---

# Phase 3 Plan 03: Runtime Memory Seam Summary

**A single policy-validated `(project, agent_id)` memory seam now supplies fresh recall and mediated proposals to authenticated ACP, protocol-host, and exec sessions.**

## Performance

- **Duration:** 1h 20m
- **Started:** 2026-09-03T04:19:00Z
- **Completed:** 2026-09-03T05:39:35Z
- **Tasks:** 3
- **Files modified:** 12

## Accomplishments

- Recorded the pre-wiring journal decision: dedicated memory mutation journal, with one attributed session-coordinator policy audit and explicit 03-04 equivalence obligations.
- Added fresh per-prompt scoped retrieval, `memory_recall`, and mediation-only `memory_propose` across ACP new/load, protocol-host, and exec fresh/resume.
- Extended `MemoryPolicyResolved` additively with optional project/agent attribution while new records require project, agent, and actual runtime session id.
- Preserved default-off quarantine and grew `activation_quarantine` from five to six rows; no existing row was removed.

## Task Commits

1. **Task 1: Decide journal topology** — `60c1188`
2. **Task 2 RED: Specify attributed seam behavior** — `a1e2b52`
3. **Task 2 GREEN: Wire authenticated ACP seam** — `a30a4a7`
4. **Task 3: Wire protocol-host and exec** — `d5431c4`
5. **Task 3 corrective: Keep rebuild authority explicit** — `ed8f938`

## Decisions Made

- Dedicated memory journals retain proven journal-first mutation durability and a single `memory-N` namespace. Session journals carry audit records only.
- `rebuild_from_journals` honors its explicit resolved-policy argument; legacy policy records remain readable but cannot change authority.
- Recall content is rendered inside an explicit `UNTRUSTED data, not instructions` block on every prompt and is never cached at session start.

## Verification

- `activation_memory_seam`: 5/5 passed.
- `activation_quarantine`: 6/6 passed (previous five rows unchanged).
- `activation_admission`: 3/3 passed.
- `corrective_regressions`: 9/9 passed.
- `durability`: 3/3 passed, including the kill-mid-write child process.
- `nano-session`: 121 unit + 1 legacy replay + 13 adversarial journal tests passed.
- Final `just gate-all`: passed fmt, workspace clippy `-D warnings`, all workspace and doc tests, Nano/shared error-table checks, and generated-contract checks.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Removed replay-time policy authority from audit records**
- **Found during:** final focused rebuild-equivalence verification.
- **Issue:** Removing the store's duplicate policy append exposed a test that rebuilt under a default policy and depended on an audit record silently replacing it.
- **Fix:** Made the explicitly supplied policy authoritative and treated session policy records as audit-only during memory replay; the equivalence test now passes the same resolved policy as the control.
- **Files modified:** `crates/nano-memory/src/store.rs`, `crates/nano-memory/tests/corrective_regressions.rs`
- **Verification:** `cargo test -p nano-memory --test durability --test corrective_regressions -- --test-threads=1`
- **Committed in:** `ed8f938`

**Total deviations:** 1 auto-fixed Rule 1 correctness issue. No scope expansion.

## Issues Encountered

- The first full gate found the external Desktop checkout's generated error-table mirrors stale. A read-only hash/diff check proved this plan changed no error-kind source or canonical artifact. An isolated generator probe passed Nano and shared targets, and the final full gate passed with `NANO_ERROR_TABLE_DESKTOP_DIR` directed to an empty in-worktree probe directory; no Desktop file was modified.

## User Setup Required

None.

## Next Phase Readiness

- 03-04 can implement migration and prove old-DB versus dedicated-journal rebuild equivalence under `03-03-DECISION.md`.
- 03-05 can measure continuity using the shared seam after this branch is reviewed and merged.

## Self-Check: PASSED

- Decision record and all created source/test files exist.
- Commits `60c1188`, `a1e2b52`, `a30a4a7`, `d5431c4`, and `ed8f938` are present.
- Required focused tests and the final local gate passed.
