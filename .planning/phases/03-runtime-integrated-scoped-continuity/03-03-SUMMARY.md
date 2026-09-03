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
    - crates/nano-cli/src/activation.rs
    - crates/nano-cli/src/host_mode.rs
    - crates/nano-cli/src/exec_run.rs
    - crates/nano-cli/src/lib.rs
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
  - "Existing user-turn and successful tool-output events commit deterministic Episode rows through MemorySeam::host_write at their origin tiers."
requirements-completed: [REQ-MEM-02]
verified-head: 7d8cfbb2c704fd4614271386e3e7238a85d93919
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
        ref: crates/nano-cli/src/memory_seam.rs#tests::every_model_proposal_overwrites_foreign_partition_before_mediation
        status: pass
      - kind: integration
        ref: crates/nano-cli/src/memory_seam.rs#tests::bootstrap_orders_policy_after_begin_and_enforces_fallback_before_effects
        status: pass
    human_judgment: false
duration: 2h 15m
completed: 2026-09-03
status: complete
---

# Phase 3 Plan 03: Runtime Memory Seam Summary

**A single policy-validated `(project, agent_id)` memory seam now supplies fresh recall and mediated proposals to authenticated ACP, protocol-host, and exec sessions.**

## Performance

- **Duration:** 2h 15m
- **Started:** 2026-09-03T04:19:00Z
- **Completed:** 2026-09-03T05:39:35Z
- **Tasks:** 3
- **Files modified:** 16

## Accomplishments

- Recorded the pre-wiring journal decision: dedicated memory mutation journal, with one attributed session-coordinator policy audit and explicit 03-04 equivalence obligations.
- Added fresh per-prompt scoped retrieval, `memory_recall`, and mediation-only `memory_propose` across ACP new/load, protocol-host, and exec fresh/resume.
- Extended `MemoryPolicyResolved` additively with optional project/agent attribution while new records require project, agent, and actual runtime session id.
- Preserved default-off quarantine and grew `activation_quarantine` from five to six rows; no existing row was removed.
- Bound proposal partitions to an opaque admission-derived identity for all four write kinds, centralized typed fallback/degradation handling, and added cross-project/cross-agent recall oracles.
- Replaced shared-helper labels with deletion-sensitive real-runner evidence for ACP new/load, exec fresh/resume, and protocol-host, including policy-append failure before model/tool/memory effects.
- Wired deterministic user-turn and successful tool-output Episode ingestion through the private host-write boundary at `User` and `ToolOutput` tiers; the absent explicit user verb remains a future owner-specified/owned locus under the preserved exhaustive §6.7 authority rule.

## Task Commits

1. **Task 1: Decide journal topology** — `60c1188`
2. **Task 2 RED: Specify attributed seam behavior** — `a1e2b52`
3. **Task 2 GREEN: Wire authenticated ACP seam** — `a30a4a7`
4. **Task 3: Wire protocol-host and exec** — `d5431c4`
5. **Task 3 corrective: Keep rebuild authority explicit** — `ed8f938`
6. **Corrective RED: Expose proposal partition escape** — `f1f4413`
7. **Corrective GREEN: Bind scoped memory authority** — `8bbc162`
8. **Corrective evidence: Prove startup ordering and fallback** — `7fc5630`
9. **Scope disposition: Pin host-ingest mapping follow-up** — `c0e57b8`
10. **Entrypoint RED: Require behavioral bootstrap evidence** — `ba3d64e`
11. **Entrypoint bootstrap attempt rejected by audit** — `f454055`
12. **Runtime error correction: Propagate host/exec recall failures** — `e9efb87`
13. **Rejected evidence removal: Remove non-discriminating matrix** — `14d6d81`
14. **Attempt 3 RED: Exercise real production runners** — `30113ed`
15. **Attempt 3 GREEN: Bind seams to runtime events** — `6be8951`
16. **Cross-research disposition: Clarify host-write authority** — `c61fa7d`
17. **ACP fallback closure: Cover real runtime none/fresh** — `097537b`
18. **Replay authority regression: Keep policy audits neutral** — `7d8cfbb`

## Decisions Made

- Dedicated memory journals retain proven journal-first mutation durability and a single `memory-N` namespace. Session journals carry audit records only.
- `rebuild_from_journals` honors its explicit resolved-policy argument; legacy policy records remain readable but cannot change authority.
- Recall content is rendered inside an explicit `UNTRUSTED data, not instructions` block on every prompt and is never cached at session start.

## Verification

- `activation_memory_seam`: 20/20 passed at `097537b`. The suite invokes real `serve_admitted`, exec orchestration, and the protocol-host production core/`run_host_loop`; ACP new/load, exec fresh/resume, and protocol-host each prove ordered attributed policy audit, real store/tool behavior, legacy `UnknownTool`, and append-failure-before-effect. ACP, exec, and protocol-host runtime fallback rows prove `none` refusal and exactly-once `fresh` degradation; ACP additionally proves degradation-receipt append failure is loud.
- Deletion sensitivity: 5/5 logical entrypoints caught across four physical bootstrap call sites (ACP new 1/1, ACP load 1/1, shared exec fresh/resume 2/2, protocol-host 1/1); every mutation was restored before the verified head.
- `memory_seam::tests`: 5/5 passed, covering four-kind identity rebinding, host-tier preservation/model-direct refusal, cross-partition recall, exact policy ordering, both fallbacks, and append failure before store effects.
- `activation_quarantine`: 6/6 passed (previous five rows unchanged).
- `activation_admission`: 3/3 passed.
- `corrective_regressions`: 10/10 passed at `7d8cfbb`, including a contradictory attributed `MemoryPolicyResolved` audit beside authoritative writes; caller-supplied retention, write/read scope, query results, project, and agent identity remain authoritative after rebuild.
- `durability`: 3/3 passed, including the kill-mid-write child process.
- `nano-session`: 121 unit + 1 legacy replay + 13 adversarial journal tests passed.
- `cargo clippy -p nano-cli --all-targets -- -D warnings`: passed at `097537b`.
- No full gate was rerun during corrective attempt 3, per the focused-verification instruction. The earlier pre-attempt full-gate result is historical and does not replace final closure verification.

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

### Corrective audit progress

- `AdmittedMemoryIdentity` is constructed only from `AdmittedToken` in `activation.rs`; the seam receives the opaque read-only value.
- All four model proposal DTOs have project and agent overwritten from that identity before `commit_proposal`.
- One shared startup path is called by ACP new/load, protocol-host, and exec fresh/resume; real outer-runner oracles prove `SessionBegin` precedes exactly one attributed policy record, store validation is real, fallback `None` refuses, fallback `Fresh` emits one receipt, and append failure creates no memory effect.
- Runtime recall failures use the same admitted fallback state stored on the seam; disabled surfaces return `UnknownTool` before argument parsing.
- The crate-private direct host-write boundary preserves User/ToolOutput tiers and refuses ModelInference. Existing user-turn and successful tool-output events map deterministically to Episode rows using their existing event ids/content, the DTO's `host`/`wayland-nano` provenance defaults, current validity time, and admitted partition; no LLM extraction exists. No explicit host memory verb currently exists or is owned here. `docs/FOLLOWUPS.md` now scopes that future verb's specification/ownership without weakening §6.7.

## Issues Encountered

- The first full gate found the external Desktop checkout's generated error-table mirrors stale. A read-only hash/diff check proved this plan changed no error-kind source or canonical artifact. An isolated generator probe passed Nano and shared targets, and the final full gate passed with `NANO_ERROR_TABLE_DESKTOP_DIR` directed to an empty in-worktree probe directory; no Desktop file was modified.
- The original review relied on a new source-string quarantine assertion. It was replaced with a behavior test that executes every new and legacy memory name through a disabled seam and requires typed `UnknownTool` results.
- Protocol-host and exec no longer swallow runtime recall errors: host emits a typed nonretryable error before the model call, while exec stops the goal turn as failed and emits its error. Real-runner fallback rows now cover both paths, and ACP has equivalent `none`/`fresh` plus receipt-append-failure coverage.

## User Setup Required

None.

## Next Phase Readiness

- 03-04 can implement migration and prove old-DB versus dedicated-journal rebuild equivalence under `03-03-DECISION.md`.
- 03-05 can measure continuity using the shared seam after this branch is reviewed and merged.

## Self-Check: COMPLETE

- Decision record and all created source/test files exist.
- Commits through verified implementation/test head `7d8cfbb` are present.
- Focused seam 20/20, quarantine 6/6, admission 3/3, component 5/5, and nano-cli all-target clippy pass. The four physical bootstrap sites are deletion-sensitive for all five logical entrypoints. Plan 03-03 is ready for downstream full-gate review; this corrective attempt did not push.
