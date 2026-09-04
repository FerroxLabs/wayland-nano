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
  - activated fork-child binding with inherited authority and rollback
  - preregistered budgets plus committed sealed NDJSON receipt evidence
  - byte-reproducible report with measured defaults recommendation
affects: [03-06, 03-07, Desktop-continuity-defaults]
tech-stack:
  added: []
  patterns: [model-request assertions behind soak feature, activated fork-child binding, canonical evidence seals]
key-files:
  created:
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
key-decisions:
  - "Recommend session_resume for interactive ACP when a valid bound session exists; use memory_recall only when continuity is requested and fresh for stateless work."
  - "Measure quality from the actual model request: fresh proves absence, fork-child resume proves replayed presence, and memory_recall proves automatic retrieved presence."
  - "Bind activated fork children from the live validated parent authority and remove the child journal if binding cannot commit."
patterns-established:
  - "Receipt rows bind seed, binary, budget, harness, task script, journal, fixture, manifest, and NDJSON digests."
  - "Committed text evidence verifies with CRLF-to-LF normalization for checkout-independent hashes."
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
        ref: "fresh-clone continuity-report.mjs regeneration at df0c1f1"
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
duration: 1h27m
completed: 2026-09-04
status: complete
---

# Phase 3 Plan 05: Continuity-mode Measurement Summary

**Causal real-binary evidence now distinguishes no continuity, activated fork-child replay, and automatic scoped recall, with preregistered budgets and committed NDJSON receipts.**

## Performance

- **Duration:** 1h 27m
- **Started:** 2026-09-04T21:39:00Z
- **Completed:** 2026-09-04T23:06:27Z
- **Tasks:** 3 plus one audit correction
- **Files modified:** 16, including four committed receipt artifacts

## Accomplishments

- Fresh creates a new admitted session with memory disabled, makes zero memory tool calls, and proves every fixture answer is absent from the actual model request: quality 0.000 by design.
- Session resume forks an activated parent, binds inherited authority before success, loads the returned `child_session_id`, and proves replayed answers are present: quality 1.000 with 8/8 typed drift refusals.
- Memory recall seeds all 50 facts and 10 decisions through four mediated partition turns, makes zero explicit memory tool calls during measurement, and proves automatic retrieval placed the expected answer into the actual request: quality 0.950.
- Token totals come exclusively from emitted `_wayland/session/budget` notices. Median totals were 5,104.5 fresh, 10,204 session resume, and 11,297 memory recall; median turn latencies were 29.909 ms, 23.352 ms, and 31.263 ms respectively.
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

## Files Created/Modified

- `crates/nano-agent/src/wiring.rs` — soak-only directive emits success only when the actual `ModelRequest` contains or excludes the specified needle; mismatch is typed.
- `crates/nano-cli/src/activation.rs` — derives a fork child binding from the validated parent and current live token.
- `crates/nano-cli/src/acp_mode.rs` — passes the active token/gate into the existing fork handler and returns typed failure on binding refusal.
- `crates/nano-cli/src/session_cmds.rs` — binds after journal creation, returns the inherited fingerprint, and removes the child on binding failure.
- `crates/nano-cli/tests/activation_memory_seam.rs` — proves exact child resume and fingerprint/project/principal drift refusals.
- `crates/nano-cli/tests/c11_session_cmds.rs` — proves binding failure leaves no child journal.
- `scripts/soak/continuity.mjs` — runs the causal three-mode battery and writes sealed evidence.
- `scripts/soak/continuity-budgets.json` — records causal budgets at `2026-09-04T22:54:18Z`, before final receipts.
- `scripts/soak/continuity-report.mjs` — validates every seal and renders measured medians and recommendations.
- `scripts/soak/test-continuity.mjs` — tests preflight, causal mode invariants, emitted usage, child resume, drift refusal, and anti-tuning behavior.
- `docs/evidence/phase3/continuity-modes-report.md` — final causal report.
- `scripts/soak/evidence/continuity-receipt-causal-final/**/{continuity-manifest.json,continuity.ndjson}` — two committed receipt packs.

## Final Receipt Manifests

Shared bindings:

- Binary source: `bcfba39b9c0d9ee3ece4069c74bea34d1df4d968`
- Binary SHA-256: `148c138bacf121913f60551f2127f186ada28a5f2a7216e981e8da7340678b7d`
- Budget SHA-256: `a394961e053bce02b59f5d0a08adad854b1307817e4f7f931e99900e5332ba6d`
- Harness SHA-256: `df19ce9000ba0dcc8aad94113722123aa6201ff4518e2f5449d2147677ccb0f9`
- Fixture SHA-256: `ad286c8ebd835667488089410b9b7bd84ecade71758b20ce678d97c3f9dda214`

Seed 1010:

- Manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T225707234Z-receipt-1010-36100/continuity-manifest.json`
- Manifest SHA-256: `ab28e4c94bc5492dc7a17c93e52d782fe85e4e1034ad1997552d86f2bec78352`
- NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T225707234Z-receipt-1010-36100/continuity.ndjson`
- NDJSON SHA-256: `9e4fde7de5c7a0cf8ed30c4b50ace173467e3dcdcd63c0e19764aa677bdbee7e`

Seed 2020:

- Manifest: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T225706996Z-receipt-2020-40936/continuity-manifest.json`
- Manifest SHA-256: `e8a0ec9d367fe9cb41e29758cf8fd6fdf4b97533d0bf56c8df3e2fe637e8da50`
- NDJSON: `scripts/soak/evidence/continuity-receipt-causal-final/run-20260904T225706996Z-receipt-2020-40936/continuity.ndjson`
- NDJSON SHA-256: `03a990e4519a64d12d0daad9252c1303f7e15b2ffc2c32224596a71beb98a062`

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

**4. [Rule 2 - Missing Critical] Committed and checkout-normalized receipt evidence**
- **Found during:** evidence review
- **Issue:** Ignored local manifests could not be independently audited, and CRLF checkout conversion could change raw text hashes.
- **Fix:** Committed only the two final manifests and two NDJSON files; report verification normalizes text line endings and records every digest.
- **Verification:** Fresh clone at `df0c1f1` regenerated the committed report byte-identically.
- **Commits:** `fecc162`, `df0c1f1`

**5. [Rule 3 - Blocking] Kept test evidence off the external F: temp volume**
- **Found during:** initial Task 1 real-binary tests
- **Issue:** F: USB latency crossed the 30-second request bound and outer timeouts left child handles.
- **Fix:** Kimi K3 and Claude Fable converged on D:-local ignored test homes plus PID-scoped process cleanup; settled request timers are cleared.
- **Verification:** Corrective real-binary suite completes in approximately 18 seconds.
- **Commits:** `78d5f5e`, `33b5243`

**Total deviations:** 5 auto-fixed. All are required for causal validity, fail-closed fork authority, reviewable evidence, or bounded execution.

## Issues Encountered

- The first receipt pack was rejected by audit because explicit recall and fixed answers made modes observationally identical. It is superseded and carries no final reference.
- Fork binding reached native attempt 3 after a return-type patch hit the neighboring existing method; an isolated signature check identified and corrected that exact variable. Focused tests then passed.
- The final automatic-recall score is 0.950 rather than 1.000. The missed row remains honest evidence; labels and budgets were not tuned.
- Repository hooks modified `AGENTS.md` and created `CLAUDE.md`; neither was staged or altered by this lane.

## Verification

- `node --test scripts/soak/test-continuity.mjs`: 5/5 pass against final binary.
- `cargo test -p nano-agent --features soak-fake-model request_assertion_directive_round_trips --lib`: pass.
- `cargo test -p nano-cli --test c11_session_cmds --test activation_memory_seam`: 3/3 and 21/21 pass.
- `cargo clippy -p nano-agent -p nano-cli --all-targets --features nano-agent/soak-fake-model -- -D warnings`: pass.
- Final receipt runs: seeds 1010 and 2020, 64 rows each, exit 0.
- Final report generation with all three required modes: pass; all budgets PASS.
- Fresh clone report regeneration at `df0c1f1`: byte-identical.
- `just gate-all` with `CARGO_TARGET_DIR=F:/CargoTarget/wayland-nano-p3`: fmt, workspace clippy `-D warnings`, all workspace/doc tests, and generated-contract checks pass.
- `git diff 628901ab -- gates/fixtures/memory-retrieval-recall-v1`: empty.

## Known Stubs

None. The request assertion exists only behind `soak-fake-model`; its mismatch is typed and no production provider path sees it.

## User Setup Required

None.

## Next Phase Readiness

- Plan 03-06 can consume committed, recomputable receipt evidence and the activated fork-child behavior.
- Desktop can evaluate the report recommendation without any Phase 3 Desktop code change.

## Self-Check: PASSED

Both committed manifests contain 64 rows and postdate the committed budget registration. Each seed proves fresh 0/20, fork-child resume 20/20, automatic recall 19/20, drift refusal 4/4, zero explicit memory calls, and positive ACP-emitted token deltas. All files and commits resolve, the fixture diff is empty, and no superseded receipt path remains in the final report or summary.

---
*Phase: 03-runtime-integrated-scoped-continuity*
*Completed: 2026-09-04*
