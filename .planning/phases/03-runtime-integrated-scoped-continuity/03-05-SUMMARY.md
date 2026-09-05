---
phase: 03-runtime-integrated-scoped-continuity
plan: 05
subsystem: continuity-measurement
tags: [acp, memory-recall, session-fork, ndjson, soak-fake-model]
requires:
  - phase: 03-03
    provides: one admitted nano-memory runtime seam across persistent entrypoints
provides:
  - causal real-binary comparison of fresh, session_resume, and memory_recall
  - one mode-independent task battery with separately hashed driver oracles
  - activated fork-child binding with inherited authority and rollback
  - preregistered budgets plus committed sealed NDJSON receipt evidence
  - byte-reproducible report with measured defaults recommendation
affects: [03-06, 03-07, Desktop-continuity-defaults]
tech-stack:
  added: []
  patterns: [model-request assertions behind soak feature, activated fork-child binding, canonical evidence seals]
key-files:
  created:
    - .gitattributes
    - scripts/soak/continuity.mjs
    - scripts/soak/continuity-budgets.json
    - scripts/soak/continuity-report.mjs
    - scripts/soak/test-continuity.mjs
    - docs/evidence/phase3/continuity-modes-report.md
  modified:
    - crates/nano-agent/src/wiring.rs
    - crates/nano-cli/src/activation.rs
    - crates/nano-cli/src/acp_mode.rs
    - crates/nano-cli/src/session_cmds.rs
    - crates/nano-cli/tests/activation_memory_seam.rs
    - crates/nano-cli/tests/c11_session_cmds.rs
    - .planning/phases/03-runtime-integrated-scoped-continuity/03-VALIDATION.md
key-decisions:
  - "Recommend session_resume for interactive ACP when a valid bound session exists; use memory_recall only when continuity is requested and fresh for stateless work."
  - "Measure quality from the actual model request: fresh proves absence, fork-child resume proves replayed presence, and memory_recall proves automatic retrieved presence."
  - "Bind activated fork children from the live validated parent authority and remove the child journal if binding cannot commit."
patterns-established:
  - "Receipt rows bind seed, binary, budget, harness, task script, journal, fixture, manifest, and NDJSON digests."
  - "One task-battery hash is shared by every mode; driver hashes differ while each query's fake usage and delay remain identical."
  - "Continuity scripts, budgets, receipts, and the report are pinned to LF so raw committed-byte hashes reproduce across checkouts."
requirements-completed: [REQ-CONT-01]
coverage:
  - id: D1
    description: "Real release binary causally distinguishes fresh, fork-child resume, and automatic memory recall over ACP."
    requirement: REQ-CONT-01
    verification:
      - kind: e2e
        ref: "node --test scripts/soak/test-continuity.mjs"
        status: pass
    human_judgment: false
  - id: D2
    description: "Activated fork children inherit exact authority and roll back on binding failure."
    requirement: REQ-CONT-01
    verification:
      - kind: integration
        ref: "crates/nano-cli/tests/activation_memory_seam.rs#activated_fork_binds_and_loads_the_returned_child_fail_closed"
        status: pass
      - kind: integration
        ref: "crates/nano-cli/tests/c11_session_cmds.rs#failed_child_binding_removes_the_fork_journal"
        status: pass
    human_judgment: false
  - id: D3
    description: "Two committed seeded receipts pass preregistered latency, emitted-token, quality, and drift budgets."
    requirement: REQ-CONT-01
    verification:
      - kind: integration
        ref: "scripts/soak/evidence/continuity-receipt-causal-final"
        status: pass
      - kind: integration
        ref: "fresh-clone continuity-report.mjs regeneration at 12f94fc"
        status: pass
    human_judgment: false
  - id: D4
    description: "The report recommends continuity defaults per surface from causal measured evidence."
    requirement: REQ-CONT-01
    verification:
      - kind: other
        ref: "docs/evidence/phase3/continuity-modes-report.md"
        status: pass
    human_judgment: true
    rationale: "Desktop owns the product default and must judge whether to adopt the recommendation."
duration: 2h52m
completed: 2026-09-05
status: complete
---

# Phase 3 Plan 05: Continuity-mode Measurement Summary

**Causal real-binary evidence now distinguishes no continuity, activated fork-child replay, and automatic scoped recall, with preregistered budgets and committed NDJSON receipts.**

## Performance

- **Duration:** 2h 52m
- **Started:** 2026-09-04T21:39:00Z
- **Completed:** 2026-09-05T00:31:07Z
- **Tasks:** 3 plus three audit corrective rounds
- **Files modified:** 19, including four committed receipt artifacts, the validation ownership contract, and scoped LF policy

## Accomplishments

- Fresh creates a new admitted session with memory disabled, makes zero memory tool calls, and proves every fixture answer is absent from the actual model request: quality 0.000 by design.
- Session resume forks an activated parent, binds inherited authority before success, loads the returned `child_session_id`, and proves replayed answers are present: quality 1.000 with 8/8 typed drift refusals.
- Memory recall seeds all 50 facts and 10 decisions through four mediated partition turns, makes zero explicit memory tool calls during measurement, and proves automatic retrieval placed the expected answer into the actual request: quality 0.950.
- One mode-independent task battery is shared across all modes. Driver/oracle scripts are separately hashed, while the same seed/query has identical fake usage and delay in every mode.
- Token totals come exclusively from emitted `_wayland/session/budget` notices. All four memory-seeding sessions and every other setup session are counted exactly once. Median setup/probe/total tokens were 0/5,136/5,136 for fresh, 5,152.5/5,136/10,288.5 for session resume, and 16,351/11,303/27,654 for memory recall. Median turn latencies were 63.012 ms fresh, 60.009 ms session resume, and 31.614 ms memory recall.
- Fresh isolation is a separate fail-closed positive oracle: all 40 absence assertions passed, while any leaked answer or protocol refusal aborts publication. Fresh continuity quality remains 0.000 by design.
- All preregistered budgets passed. The report recommends session resume for a valid bound interactive session, memory recall only when continuity is requested, and fresh for stateless work.

## Task Commits

Initial implementation:

1. `9383522` — define continuity harness behavior
2. `78d5f5e` — implement real-binary ACP harness
3. `e2983d5` — freeze initial budgets and reporter
4. `33b5243` — publish initial measurement
5. `5d55406` — initial summary

Audit correction:

1. `b725f28` — authorize causal soak assertion
2. `a14294d` — add causal RED tests
3. `1e7e49b` — authorize fork-child binding correction
4. `0606981` — add soak-only actual-request assertion
5. `f3be675` — add fork-child RED tests
6. `ce9f7a2` — bind activated fork children
7. `bcfba39` — format rollback assertion
8. `a9fe3e5` — make the three modes causally distinct
9. `0cead83` — preregister causal budgets before receipt runs
10. `d689583` — reduce fixture seeding to one turn per partition
11. `fecc162` — commit causal manifests, NDJSON, and report
12. `df0c1f1` — normalize evidence hashes for fresh Windows checkouts
13. `01f4a98` — add controlled-comparison and active-target RED tests
14. `be5e7cb` — restrict activated forks to the active session
15. `98144bc` — separate task battery, driver oracle, and probe accounting
16. `768427b` — preregister like-for-like probe budgets
17. `36d9aa0` — amend validation ownership and disjointness
18. `173d72b` — declare receipt and validation ownership in the plan
19. `852e6ae` — bind the common task hash to all rows and remove old receipts
20. `3e9c545` — commit controlled-comparison receipts and report

Complete-accounting correction:

1. `96b46f5` — require conserved setup and fresh-isolation RED tests
2. `76a52bb` — account for all ACP-emitted setup and probe usage
3. `02114a8` — pin continuity evidence surfaces to LF
4. `a7d4a5e` — preregister complete total-token ceilings
5. `12f94fc` — replace receipts and report with complete accounting

## Files Created/Modified

- `crates/nano-agent/src/wiring.rs` — soak-only directive emits success only when the actual `ModelRequest` contains or excludes the specified needle; mismatch is typed.
- `crates/nano-cli/src/activation.rs` — derives a fork child binding from the validated parent and current live token.
- `crates/nano-cli/src/acp_mode.rs` — passes the active token/gate into the existing fork handler and returns typed failure on binding refusal.
- `crates/nano-cli/src/session_cmds.rs` — binds after journal creation, returns the inherited fingerprint, and removes the child on binding failure.
- `crates/nano-cli/tests/activation_memory_seam.rs` — proves exact child resume and fingerprint/project/principal drift refusals.
- `crates/nano-cli/tests/c11_session_cmds.rs` — proves binding failure leaves no child journal.
- `scripts/soak/continuity.mjs` — runs the causal three-mode battery and writes sealed evidence.
- `scripts/soak/continuity-budgets.json` — records complete setup, probe, and total-token budgets at `2026-09-05T00:05:52Z`, before final receipts.
- `scripts/soak/continuity-report.mjs` — validates every seal and renders measured medians and recommendations.
- `scripts/soak/test-continuity.mjs` — tests preflight, causal mode invariants, emitted usage, child resume, drift refusal, and anti-tuning behavior.
- `docs/evidence/phase3/continuity-modes-report.md` — final causal report.
- `scripts/soak/evidence/continuity-receipt-causal-final/**/{continuity-manifest.json,continuity.ndjson}` — two committed receipt packs.
- `.planning/phases/03-runtime-integrated-scoped-continuity/03-VALIDATION.md` — records approved Rust correction ownership and continued 03-04/03-05 disjointness.
- `.gitattributes` — pins only the owned continuity scripts, budget, evidence, and report to LF for raw-byte reproducibility.

## Final Receipt Manifests

Shared bindings:

- Binary source: `36d9aa0d4f68543841b5f800b518d4673299d8da`
- Binary SHA-256: `fba0a81b552da7904e1a713e3bf9cbe6da5f88bd0debcc0ba984ef5c8b685933`
- Budget SHA-256: `59e7924bebd93fd2ef3e9a65a4c0cb8177c382bc3484d0e6f8ad5fdabf8ff320`
- Harness SHA-256: `40fc1531154586fd0d2fdafe9791d9998bfe0cc0804a6a8508fce8f118610ad0`
- Fixture SHA-256: `ad286c8ebd835667488089410b9b7bd84ecade71758b20ce678d97c3f9dda214`
- Task battery SHA-256: `abad3826d6c49c0e0cad6b694180abc4b3e523ddce4833b51c5caa04190ab7f0`

Seed 1010:

- Manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260905T000637916Z-receipt-1010-8916/continuity-manifest.json`
- Manifest SHA-256: `0c507e4bdd47108dcdd0c51eecce44a6142b0cf7c413967e10db6ca874c4b6b7`
- NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260905T000637916Z-receipt-1010-8916/continuity.ndjson`
- NDJSON SHA-256: `e94d95df8f19d660b2bb284cda902c67c1397645e6edf9db778dbc3a020f24f7`

Seed 2020:

- Manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260905T000638217Z-receipt-2020-16516/continuity-manifest.json`
- Manifest SHA-256: `d51940957310c4d502f0e9236fa8f0659394445ff3348b156410acd08da5fbcd`
- NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260905T000638217Z-receipt-2020-16516/continuity.ndjson`
- NDJSON SHA-256: `1eb7c12a31d5c9f0891d9241e1ae79d6f91061825adbeb3df2be25fc319b9daa`

The final report raw SHA-256 is `ffa71044bed0792ecf35ff53ec2de61f275dddedf91243e1140f5de9134b80a3`. Budgets were preregistered at `2026-09-05T00:05:52Z`; both replacement receipts started afterward.

No final report or summary references the superseded noncausal receipts.

## Decisions Made

- The soak fake model gained one feature-gated request assertion because the old driver ignored `ModelRequest`, making causal continuity measurement impossible from scripts alone.
- Activated forks inherit every binding field from the validated parent. A caller cannot provide child authority, and any binding failure removes the created child before fork success is returned.
- Fresh quality is zero for a recall battery by design; it is not awarded points for a scripted answer. Resume and recall score only when the expected fixture answer is present in the actual model request.
- The report remains a recommendation input. Desktop retains default-setting authority.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Added causal model-request observation**
- **Found during:** independent audit of the initial receipt pack
- **Issue:** All three modes called the same explicit recall tool and the scripted expected answer made quality self-fulfilling.
- **Fix:** Added the feature-gated `assert_request` directive and mode-specific causal oracles; production provider behavior is unchanged without `soak-fake-model`.
- **Verification:** Rust directive round-trip/request tests plus real-binary Node suite 5/5.
- **Commits:** `b725f28`, `a14294d`, `0606981`, `a9fe3e5`

**2. [Rule 2 - Missing Critical] Bound and resumed the returned activated fork child**
- **Found during:** causal session-resume design
- **Issue:** Fork created a journal but no activation binding, so only the parent could resume.
- **Fix:** Derive child binding from the live validated parent/token, append it before returning success, return the inherited fingerprint, and remove the child journal on binding failure.
- **Verification:** Activated child load succeeds; fingerprint, project, and principal drift refuse; injected binding failure leaves one parent journal only.
- **Commits:** `1e7e49b`, `f3be675`, `ce9f7a2`, `bcfba39`

**3. [Rule 1 - Bug] Removed seeding contamination from the recall battery**
- **Found during:** first causal receipt report
- **Issue:** The previous timeout workaround used one prompt per row, adding 60 high-trust host episodes and lowering automatic recall to 0.850.
- **Fix:** Seed each `(project, agent_id)` partition in one mediated turn, adding only four setup episodes while preserving all 60 curated rows.
- **Verification:** Final memory-recall quality is 0.950; fixture/label diff remains empty.
- **Commit:** `d689583`

**4. [Rule 2 - Missing Critical] Committed and byte-stable receipt evidence**
- **Found during:** evidence review
- **Issue:** Ignored local manifests could not be independently audited, and CRLF checkout conversion could change raw text hashes.
- **Fix:** Committed only the two final manifests and two NDJSON files, then scoped LF policy to the owned continuity evidence surfaces so raw bytes and digests are checkout-stable.
- **Verification:** Fresh clone at `12f94fc` regenerated the committed report byte-identically twice with raw SHA-256 `ffa71044bed0792ecf35ff53ec2de61f275dddedf91243e1140f5de9134b80a3` and no CR bytes.
- **Commits:** `fecc162`, `df0c1f1`, `02114a8`, `12f94fc`

**5. [Rule 3 - Blocking] Kept test evidence off the external F: temp volume**
- **Found during:** initial Task 1 real-binary tests
- **Issue:** F: USB latency crossed the 30-second request bound and outer timeouts left child handles.
- **Fix:** Kimi K3 and Claude Fable converged on D:-local ignored test homes plus PID-scoped process cleanup; settled request timers are cleared.
- **Verification:** Corrective real-binary suite completes in approximately 18 seconds.
- **Commits:** `78d5f5e`, `33b5243`

**6. [Rule 2 - Missing Critical] Restricted activated forks to the active session**
- **Found during:** attempt-3 authority audit
- **Issue:** A live session could name and fork a closed same-identity session because identity equality alone authorized the target.
- **Fix:** Activated fork requests now require the target to equal the host's active session before any journal copy or child binding.
- **Verification:** The two-session negative returns typed `session_fork_failed` and leaves the journal/binding inventory unchanged.
- **Commits:** `01f4a98`, `be5e7cb`

**7. [Rule 1 - Bug] Removed task-script and inherited-probe confounds**
- **Found during:** attempt-3 controlled-comparison audit
- **Issue:** Mode-specific scripts changed fake usage/delay, and the first resumed probe included parent-history tokens.
- **Fix:** Hash one task battery for all modes, hash driver/oracle scripts separately, generate identical per-query fake profiles, and subtract the inherited child meter baseline while reporting setup tokens independently.
- **Verification:** Every seed/label has one task hash, three driver hashes, one fake profile, and positive like-for-like probe deltas.
- **Commits:** `98144bc`, `768427b`, `852e6ae`, `3e9c545`

**8. [Rule 2 - Missing Critical] Updated validation ownership**
- **Found during:** attempt-3 governance audit
- **Issue:** The lane table still described 03-05 as scripts/docs-only after approved Rust corrections.
- **Fix:** Added the soak directive, fork-binding sources/tests, and committed receipt paths while preserving 03-04 disjointness and recording sequential overlap with merged 03-03.
- **Verification:** Plan and validation ownership lists cover all 19 changed paths.
- **Commits:** `36d9aa0`, `173d72b`

**9. [Rule 1 - Bug] Conserved every emitted setup token and made fresh isolation fail closed**
- **Found during:** strike-3 comparable-accounting audit
- **Issue:** Four memory-seeding sessions were omitted from costs, total-token ceilings were not preregistered, and fresh could treat leakage/refusal as ordinary zero quality.
- **Cross-research:** Kimi K3 session `session_7de07d0f-05ac-42c8-a091-dbf9765a1909` and Claude Fable independently converged on single attribution of emitted setup usage, explicit `setup + probe = total` conservation, preregistered total ceilings, and a separate fail-closed freshness oracle. The reconciled minimal scheme attributes setup once to the first probe row in each mode partition.
- **Fix:** Capture every ACP budget notice, bind setup session ids, conserve row and manifest totals, abort on fresh leakage or refusal, preregister per-mode total ceilings, and regenerate both receipts.
- **Verification:** Node RED/green suite is 6/6; fresh leak injection exits nonzero without publishing `latest.json`; each manifest conserves all row usage and postdates the budget registration.
- **Commits:** `96b46f5`, `76a52bb`, `a7d4a5e`, `12f94fc`

**Total deviations:** 9 auto-fixed. All are required for causal validity, complete comparable accounting, fail-closed fork authority, controlled comparison, reviewable evidence, or bounded execution.

## Issues Encountered

- The first receipt pack was rejected by audit because explicit recall and fixed answers made modes observationally identical. It is superseded and carries no final reference.
- Attempt-3 rejected three different task hashes and mode-dependent fake profiles. The final receipts use one task hash and identical usage/delay per seed/query; driver hashes alone differ.
- Fork binding reached native attempt 3 after a return-type patch hit the neighboring existing method; an isolated signature check identified and corrected that exact variable. Focused tests then passed.
- Reporter attempts correctly rejected the old budget manifests and drift rows missing the common task hash. Superseded tracked receipts were removed, the common write boundary was fixed, and only final manifests remain committed.
- The final workspace gate encountered a shared-target collision after fmt, clippy, and all preceding tests were green: `mem_sec_gate_summary` loaded a prebuilt harness compiled from the concurrent `.tmp-wt-p3-migration` worktree. The cheapest distinguishing rerun rebuilt only that exact predicate in `F:/CargoTarget/wayland-nano-p3-0305-isolated` and passed `gate: 6/6`, proving cache contamination rather than a source failure. This follows the plan's explicit parallel-lane fallback to scope to owned crates after a transient workspace failure.
- The final automatic-recall score is 0.950 rather than 1.000. The missed row remains honest evidence; labels and budgets were not tuned.
- Repository hooks modified `AGENTS.md` and created `CLAUDE.md`; neither was staged or altered by this lane.

## Verification

- `node --test scripts/soak/test-continuity.mjs`: 6/6 pass against final binary, including setup-token omission and fresh-leak publication negatives.
- `cargo test -p nano-agent --features soak-fake-model request_assertion_directive_round_trips --lib`: pass.
- `cargo test -p nano-cli --test c11_session_cmds --test activation_memory_seam`: 3/3 and 22/22 pass.
- `cargo clippy -p nano-agent -p nano-cli --all-targets --features nano-agent/soak-fake-model -- -D warnings`: pass.
- Final receipt runs: seeds 1010 and 2020, 64 rows each, exit 0.
- Final report generation with all three required modes: pass; all budgets PASS.
- Fresh clone report regeneration at `12f94fc`: raw-LF byte-identical on two consecutive regenerations; report SHA-256 `ffa71044bed0792ecf35ff53ec2de61f275dddedf91243e1140f5de9134b80a3`.
- `just gate-all` with `CARGO_TARGET_DIR=F:/CargoTarget/wayland-nano-p3`: fmt and workspace clippy `-D warnings` passed; all workspace tests preceding mem-sec passed. The sole red was the externally contaminated shared-target prebuilt described above.
- `cargo test --locked -p nano-memory --test mem_sec_cards mem_sec_gate_summary -- --exact --nocapture --test-threads=1` with isolated `F:/CargoTarget/wayland-nano-p3-0305-isolated`: pass, `gate: 6/6`.
- `git diff 628901ab -- gates/fixtures/memory-retrieval-recall-v1`: empty.

## Known Stubs

None. The request assertion exists only behind `soak-fake-model`; its mismatch is typed and no production provider path sees it.

## User Setup Required

None.

## Next Phase Readiness

- Plan 03-06 can consume committed, recomputable receipt evidence and the activated fork-child behavior.
- Desktop can evaluate the report recommendation without any Phase 3 Desktop code change.

## Self-Check: PASSED

The branch changes exactly 19 declared files and tracks exactly two final manifests plus two NDJSON packs. Each seed has one task-battery hash, three driver hashes per label, one identical fake profile per label, 20/20 fresh isolation assertions with fresh continuity quality 0, fork-child resume 20/20, automatic recall 19/20, four memory-seed setup allocations, equal fresh/resume probe usage, and 4/4 drift refusals. Every row and manifest conserves `total_tokens = setup_tokens + probe_tokens`; both manifests postdate budget registration, the LF-pinned report recomputes without a byte diff, and the frozen fixture is unchanged.

---
*Phase: 03-runtime-integrated-scoped-continuity*
*Completed: 2026-09-05*
