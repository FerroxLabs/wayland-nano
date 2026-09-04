---
phase: 03-runtime-integrated-scoped-continuity
plan: 05
subsystem: continuity-measurement
tags: [acp, memory, session-resume, ndjson, soak-fake-model]
requires:
  - phase: 03-03
    provides: one admitted nano-memory runtime seam across persistent entrypoints
provides:
  - seeded real-binary comparison of fresh, session_resume, and memory_recall
  - canonical pre-registered continuity budgets and tamper-refusing report generator
  - two-seed receipt report with typed resume-drift evidence and default recommendations
affects: [03-06, 03-07, Desktop-continuity-defaults]
tech-stack:
  added: []
  patterns: [canonical-json budget seals, LF-normalized harness seals, seeded ACP fake-model measurement]
key-files:
  created:
    - scripts/soak/continuity.mjs
    - scripts/soak/continuity-budgets.json
    - scripts/soak/continuity-report.mjs
    - scripts/soak/test-continuity.mjs
    - docs/evidence/phase3/continuity-modes-report.md
  modified: []
key-decisions:
  - "Recommend session_resume for interactive ACP when a valid bound session exists; use fresh otherwise."
  - "Recommend fresh for one-shot exec; keep memory_recall opt-in because it did not beat session_resume on measured quality per token."
  - "Treat the fake-model result as continuity-plumbing evidence, not semantic reasoning quality, because ACP intentionally exposes tool-result digests only."
patterns-established:
  - "Evidence rows bind seed, binary, budget, harness, task script, journal, and NDJSON digests."
  - "Budget hashes use canonical JSON and harness hashes normalize CRLF to LF for checkout-independent verification."
requirements-completed: [REQ-CONT-01]
coverage:
  - id: D1
    description: "Real release binary exercises fresh, session_resume, and memory_recall over ACP with a frozen seeded task battery."
    requirement: REQ-CONT-01
    verification:
      - kind: e2e
        ref: "node --test scripts/soak/test-continuity.mjs"
        status: pass
    human_judgment: false
  - id: D2
    description: "Budget and harness seals make post-run tuning or evidence substitution fail closed."
    requirement: REQ-CONT-01
    verification:
      - kind: integration
        ref: "scripts/soak/test-continuity.mjs#report refuses evidence bound to a different budget hash"
        status: pass
      - kind: integration
        ref: "node scripts/soak/continuity-report.mjs --evidence-dir scripts/soak/evidence/continuity-receipt-final --require-modes fresh,session_resume,memory_recall"
        status: pass
    human_judgment: false
  - id: D3
    description: "The report recommends defaults per surface from measured latency, tokens, quality, and drift behavior."
    requirement: REQ-CONT-01
    verification:
      - kind: other
        ref: "docs/evidence/phase3/continuity-modes-report.md"
        status: pass
    human_judgment: true
    rationale: "Desktop owns the product default and must judge whether to adopt this measurement recommendation."
duration: 52min
completed: 2026-09-04
status: complete
---

# Phase 3 Plan 05: Continuity-mode Measurement Summary

**A real release binary measured all three continuity strategies over seeded ACP runs, with frozen budgets, typed drift refusals, and hash-bound report generation.**

## Performance

- **Duration:** 52 min
- **Started:** 2026-09-04T21:39:00Z
- **Completed:** 2026-09-04T22:31:00Z
- **Tasks:** 3
- **Files modified:** 5

## Accomplishments

- Drove the real `wayland-nano.exe` built with `nano-agent/soak-fake-model` through fresh, session-resume, and memory-recall ACP carriers while seeding all 50 facts and 10 decisions through mediated memory proposals.
- Pre-registered per-mode latency, token, and quality budgets before receipt runs and bound every accepted row to canonical budget, normalized harness, binary, task-script, journal, fixture, and NDJSON digests.
- Published two-seed results: every mode scored 1.000 quality at 8,000 tokens; median turn latency was 46.685 ms fresh, 47.087 ms session_resume, and 47.780 ms memory_recall; all budgets passed.
- Proved 8/8 drifted resume attempts returned typed `resume_drift` refusals with zero silent fallbacks.

## Task Commits

1. **Task 1 RED: Define continuity harness behavior** — `9383522`
2. **Task 1 GREEN: Measure continuity modes through ACP** — `78d5f5e`
3. **Task 2: Freeze continuity budgets and reports** — `e2983d5`
4. **Task 3: Publish continuity mode measurement** — `33b5243`

## Files Created/Modified

- `scripts/soak/continuity.mjs` — provisions artifact-bound test authority, seeds the frozen fixture, drives real ACP processes, measures turns, and emits NDJSON/manifests.
- `scripts/soak/continuity-budgets.json` — canonical pre-run ceilings for latency, tokens, and quality.
- `scripts/soak/continuity-report.mjs` — validates seals and renders medians, verdicts, manifests, drift counts, and recommendations.
- `scripts/soak/test-continuity.mjs` — real-binary preflight/mode/drift tests plus the budget-tuning negative.
- `docs/evidence/phase3/continuity-modes-report.md` — published two-seed evidence and Desktop recommendation input.

## Receipt Manifests

Both runs used binary SHA-256 `376644e63782422e9bf3f4143095efe6880e070c97a70537982e0827445905e9`, compiled source `9383522be3ef80e039aad885b95be9922a18d5f2`, budget SHA-256 `01c267c0d14cbcce7a97c2db9ca6d33bd149685fd4d18495b903d56f7a8b2fbe`, and harness SHA-256 `1a9b877064ae235358a2817f554dd8a969222173840c6c732f0a1ab1068f8a46`.

- Seed 1010: `scripts/soak/evidence/continuity-receipt-final/run-20260904T222034263Z-receipt-1010-40432/continuity-manifest.json`; 64 rows in its sibling `continuity.ndjson`.
- Seed 2020: `scripts/soak/evidence/continuity-receipt-final/run-20260904T222034100Z-receipt-2020-41204/continuity-manifest.json`; 64 rows in its sibling `continuity.ndjson`.

The evidence directory is the existing ignored soak artifact store. The committed report records each path and digest; the harness deterministically reproduces the pack from the frozen fixture.

## Decisions Made

- Interactive ACP should resume a valid bound session and otherwise start fresh; the 8/8 drift refusal result keeps this fail closed.
- One-shot exec should start fresh. Memory recall remains opt-in because its measured quality per token equaled session resume rather than exceeding it.
- The quality score combines correct partitioned seed evidence, a nonempty real recall-tool result digest, and the deterministic fixture-derived answer. Digest-only ACP intentionally prevents the client from claiming it inspected raw retrieval content; mem-sec and retrieval-recall tests retain that responsibility.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Kept test evidence off the external F: temp volume**
- **Found during:** Task 1 real-binary test execution
- **Issue:** Three runs timed out at different ACP request boundaries when `mkdtemp(os.tmpdir())` placed journals on the external USB F: volume; an outer timeout also left a child holding the directory.
- **Fix:** After mandatory Kimi K3 and Claude Fable cross-research converged on the volume variable, test homes moved under the existing ignored D: soak-evidence root and PID-scoped process-tree cleanup was added.
- **Files modified:** `scripts/soak/continuity.mjs`, `scripts/soak/test-continuity.mjs`
- **Verification:** Real-binary Node suite passed 5/5 in 31.4 seconds.
- **Committed in:** `78d5f5e` and `33b5243`

**2. [Rule 2 - Missing Critical] Bound report inputs to stable source digests**
- **Found during:** Task 3 evidence review
- **Issue:** Initial exploratory manifests bound the budget bytes but not the harness, which was insufficient to exclude post-run harness edits and was line-ending-sensitive.
- **Fix:** Canonicalized budget JSON, normalized harness line endings before hashing, bound both hashes into every row/manifest, and made the reporter reject mismatches and cross-mode task-script divergence.
- **Files modified:** `scripts/soak/continuity.mjs`, `scripts/soak/continuity-report.mjs`, `scripts/soak/test-continuity.mjs`
- **Verification:** Final receipt manifests share the recorded budget/harness seals; report generation and mutation-negative test pass.
- **Committed in:** `33b5243`

**3. [Rule 1 - Bug] Cleared settled ACP request timers**
- **Found during:** Task 3 receipt execution
- **Issue:** Promise-race timeout timers survived successful requests, keeping Node alive for 30 seconds after evidence was complete.
- **Fix:** Each request now clears its timer on resolution or rejection.
- **Files modified:** `scripts/soak/continuity.mjs`
- **Verification:** Parallel receipt seeds completed in about 31 seconds each rather than waiting after manifest publication.
- **Committed in:** `33b5243`

**Total deviations:** 3 auto-fixed (1 blocking environment issue, 1 missing evidence control, 1 timer bug). **Impact on plan:** All changes strengthen reproducibility or remove measured wall-clock waste; no product/runtime surface changed.

## Issues Encountered

- The fake-model seam cannot inspect its request, and ACP deliberately exposes tool outputs as digests. The report therefore describes its quality score as continuity-plumbing evidence and makes no semantic reasoning claim.
- Kimi K3 session `session_85f9a8c9-7abd-4a93-a4ca-ebee94bf132d` and Claude Fable independently ranked D:-local evidence plus scoped process cleanup above increasing the 30-second request timeout. The timeout remains unchanged.
- Repository hooks modified `AGENTS.md` and created `CLAUDE.md` in this worktree. They were never staged or altered by this lane.

## Verification

- `node --test scripts/soak/test-continuity.mjs`: 5/5 pass.
- Two `--mode receipt` runs for seeds 1010 and 2020: 64 rows each, exit 0.
- `continuity-report.mjs --require-modes fresh,session_resume,memory_recall`: pass over both receipt manifests.
- Budget-hash mismatch test: typed refusal before report rendering.
- `just gate-all` with `CARGO_TARGET_DIR=F:/CargoTarget/wayland-nano-p3`: fmt, clippy `-D warnings`, workspace tests, doc tests, and generated-contract checks pass.
- `git diff 628901ab -- gates/fixtures/memory-retrieval-recall-v1`: empty; fixture rows and labels unchanged.

## Known Stubs

None. `FLUX_API_KEY=wayland-nano-continuity-placeholder` is the existing fake-model seam credential pattern and never reaches a network provider.

## User Setup Required

None.

## Next Phase Readiness

- Plan 03-06 can consume the committed report and repeat the deterministic receipt/report commands in closure evidence.
- Desktop must decide whether and where to adopt the recommendation. This plan found no evidence authorizing a Desktop configuration change and did not touch Desktop paths.

## Self-Check: PASSED

All five deliverable files and this summary exist; all four task commits resolve; the two receipt manifests contain 64 rows each; the report contains both seeds, the 8/8 drift result, and its recommendation; the frozen fixture diff is empty.

---
*Phase: 03-runtime-integrated-scoped-continuity*
*Completed: 2026-09-04*
