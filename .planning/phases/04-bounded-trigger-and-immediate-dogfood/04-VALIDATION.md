# Phase 4 Nyquist Validation Strategy

## Principle

Every trust claim has a failing-before-implementation gate, an external oracle, and a final exact-artifact repetition. Source grep, mocks, runtime self-report and a green compiler are never sufficient. Each behavior below names its first owning plan and final closure in Plan 04-07, with independent verdict reserved to the ferrox-verifier inside 04-07 Task 3.

Authority: 04-CONTEXT (D4-01..D4-09), 04-RESEARCH, MEMORY-CONTRACT v1.2, Phase 2/3 verifications (PASS).

## Per-Plan Contract / Harness Table

| Contract / harness | First owner | Required failing evidence before production code | Final oracle |
|---|---:|---|---|
| Scheduler inventory + dispositions (incl. jobs.json) | 01 | Sole-fire/absence assertions red against the not-yet-removed wiring | 04-07: quarantine suite green on merged tree; verifier reruns from fresh checkout |
| Sole-fire negative (structural absence) | 01 | Red pre-removal; forced cronjob call, env canary, ticker construction all proven absent post-removal | Source-grep + behavioral rows green; activation_legacy_replay green unmodified |
| Typed unknown_outcome on receipts | 02 | Crash-fault rows fail while ambiguity surfaces only as tool-refusal text | activation_effects rows green; external effects.jsonl oracle |
| Manual reconciliation surface | 02 | list/resolve rows fail before effect_cmds exists | effect_reconciliation process rows green; no redispatch path |
| No-exact-once wording | 02 | Banned-pattern grep red on surviving claims | Executable banned-pattern check green in test target |
| Deterministic occurrence keys (Desktop) | 03 | Cross-instance byte-identity tests fail under randomId() | Desktop unit suite green in .tmp-wt-phase4; restart-dedup exercised in 04-07's cross-repo run |
| Admission tuple dedup / receipt replay / conflict | 03 | Duplicate-occurrence and changed-bytes rows fail or are unproven | admission_matrix + trigger_idempotency green; zero-side-effect process oracle |
| Cancel/pause + bounded retry for triggered runs | 04 | Control-ordering and retry-storm rows fail before the cap exists | trigger_controls + admission_matrix green; typed receipts |
| Greenfield escalation + emergency refusal | 04 | Round-trip and flood rows fail before escalation.rs exists | controls_bounded green; journal-semantics.json git-diff byte-identity |
| Run-record retention + tombstone decision | 04 | Cap-overflow and sweep rows fail before run_retention.rs exists | controls_bounded green; ledgers provably append-only; decision recorded |
| Triggered-run records + scoped recall | 05 | Projection rebuild-equivalence and recall rows fail before run_records.rs exists | triggered_runs green; memory_recall legs green-or-BLOCKED with gate note, never simulated |
| Activation-aware dogfood harness | 06 | Preflight rejects unmarked binary; unauthenticated-home row fails closed | Multi-seed receipt runs + dogfood report; verifier re-derives rows from NDJSON |

## Requirement Test Matrix

| Requirement behavior | Focused command | Closure |
|---|---|---|
| REQ-RUN-01: scheduler retired, quarantine gate rewritten not weakened, Cron* replay intact | `cargo test -p nano-cli --test activation_quarantine --test trigger_idempotency -- --test-threads=1` + `cargo test -p nano-session --test activation_legacy_replay -- --test-threads=1` | 7-leg CI + verifier fresh-checkout rerun (04-07) |
| REQ-RUN-01: idempotent Desktop-triggered activations, immutable request binding | `cargo test -p nano-activation --test admission_matrix -- --test-threads=1` + Desktop `bun run test:vitest -- tests/unit/process/agent/activation/waylandNanoActivation.test.ts` | Exact-artifact cross-repo run incl. restart-dedup through both ACP stacks (04-07) |
| REQ-RUN-01: unknown_outcome + manual reconciliation, no exact-once claim | `cargo test -p nano-agent --test activation_effects -- --test-threads=1` + `cargo test -p nano-cli --test effect_reconciliation -- --test-threads=1` | Crash-ambiguity dogfood rows (04-06) + verifier (04-07) |
| REQ-RUN-01: cancel/pause, bounded retry, retention, escalation, emergency refusal | `cargo test -p nano-activation --test controls_bounded -- --test-threads=1` + `cargo test -p nano-cli --test trigger_controls -- --test-threads=1` | journal-semantics.json byte-identity + verifier (04-07) |
| REQ-RUN-01: Nano stores no schedule or timer | Sole-fire rows in `activation_quarantine` + `trigger_idempotency` + dogfood sole-fire row | Verifier source-grep + behavioral rerun (04-07) |
| REQ-DOG-01: repeated interactive + scheduled dogfood, recall/denial/replay/remap/cancel/crash proofs | `node scripts/soak/dogfood.mjs --mode receipt --seed <s>` + `node scripts/soak/dogfood-report.mjs --evidence-dir scripts/soak/evidence --out docs/evidence/phase4/dogfood-report.md` | Dogfood report + owner decision package + verifier NDJSON re-derivation (04-07) |
| REQ-DOG-01: owner accept/revise/reject recorded | Evidence: docs/evidence/phase4/owner-decision-package.md | Verifier confirms the decision exists and is owner-recorded (04-07) |

## Security Mutation Matrix (trigger-specific)

- Occurrence-key confusion/collision: same instant different routine_id; same routine different instant; tuple-component swaps (issuer/principal/project); minute-truncation boundary instants (:59→:00 crossover); cross-process derivation byte-identity.
- Replayed/duplicated triggers: byte-identical carrier twice (receipt replay, zero re-execution); same key with changed bytes (IdempotencyConflict, terminal); duplicate across Desktop restart (deterministic keys dedup); duplicate after crash between admission journal and receipt.
- Cross-routine idempotency theft: routine A's occurrence key presented under routine B's binding; logicalActivationId reuse across routines; replay-assertion cache poisoning with changed payload (WaylandNanoActivationRetryConflictError, terminal).
- Effect ambiguity laundering: crash after dispatch ⇒ UnknownOutcome + receipt, never silent success; reconciliation resolve that attempts redispatch (no such path); resolve with wrong/unknown effect_id; re-resolve of resolved row; wording surfaces claiming exactly-once/at-least-once.
- Escalation flooding / rate-limit bypass: N cap-exceeded events in one window ⇒ one attention record + journaled suppression count; split across routines/agents to dodge per-key caps; suppression must never be a silent drop.
- Retention-cap escape: insert past run-record/receipt caps (oldest evicted from projection only); eviction during an active activation (untouched); ledger rows never deleted; tombstone sweep of an unexpired or in-window tombstone (typed refusal / not swept); replay of a nonce whose tombstone was legitimately expired past the maximum authority window (semantics preserved per the 04-04 decision).
- Retirement authority-regain: forced cronjob tool call (UnknownTool — no executor exists); NANO_CRON_ENABLED set (no reader); hand-written Op::CronFired in a journal (replay no-op, grants no authority); jobs.json hand-edited (inert — no reader); any source path constructing a ticker (absence assertions red).
- Seam/gate integrity: memory_recall dogfood legs simulated or stubbed while the Phase 3 gate is closed (forbidden — recorded BLOCKED instead); carrier minted outside the fixture issuer (typed refusal); dogfood home without enablement (fails closed before session creation).

## Wave Gates

1. Wave 1 (04-01, 04-02) is parallel and Phase-3-independent per 04-RESEARCH §7: retirement touches cron/acp/exec/quarantine surfaces; effects work touches activation_effects/mcp/receipt/effect_cmds. The shared file hazard is docs/SEVERITY-MAP.md — owned by 04-01; 04-02's wording audit explicitly excludes it this wave.
2. Wave 2 (04-03, 04-04) is sequential on Wave 1: 04-03's sole-fire composite requires 04-01's merged removal, and its reconciliation-aware trigger tests assume 04-02's receipt surfacing; 04-04's retry-cap escalation composes 04-03's occurrence keys. 04-03's Desktop half runs only in the authorized .tmp-wt-phase4 worktree (D4-09/D2-13), Nano PR landing first.
3. Wave 3 (04-05) follows 04-04 (retention machinery) and records the Phase 3 runtime gate for memory_recall legs: green-or-BLOCKED, never simulated; fresh/session_resume legs are complete regardless.
4. Wave 4 (04-06) follows 04-03 (occurrence derivation for scheduled legs) and 04-05 (run records/recall); the chassis provisions authenticated homes via phase2_fixture on every run.
5. Wave 5 (04-07) closes: gates + 7-leg CI + exact-artifact cross-repo run + disclosed-governance merges + owner decision package, then the fresh-checkout ferrox-verifier authors 04-VERIFICATION.md and ledgers flip only on an all-VERIFIED PASS, recording v1.1 milestone-complete.
6. Cross-phase merge order (D4-07 declared hazard): 04-01 lands after 03-04 merges, or rebases onto 03-04's merged activation_quarantine.rs rows before rewriting the cron source-text assertions; both plans extend, never delete, each other's rows. The same rule applies transitively to 04-03/04-05 quarantine-file consumers.

## File Ownership Conflict Audit

| Plan | Lane | Exclusive write surface | Overlap result |
|---:|---|---|---|
| 01 | scheduler-retirement | docs/SCHEDULER-RETIREMENT.md, cron.rs, cron_fire.rs, lib.rs, acp_mode.rs (cron arms), exec_run.rs (cron wrapper), exec_mode.rs, exec_tests.rs, cron_tests.rs, fork_tests.rs, activation_quarantine.rs (cron rewrite), scripts/s6-proof/, UPSTREAM.md, docs/SEVERITY-MAP.md | acp_mode.rs/exec_run.rs shared with merged Phase 3 by merge order (lands after 03-04; rebase rule above). activation_quarantine.rs: cross-phase merge-order note — see Wave Gates item 6. |
| 02 | effects | activation_effects.rs, mcp.rs, nano-agent effect tests, admission.rs (unknown_outcome wiring), receipt.rs, effect_cmds.rs, main.rs, effect_reconciliation.rs | admission.rs/receipt.rs shared with 04-04 by wave separation (02 Wave 1, 04 Wave 2). SEVERITY-MAP.md excluded from 02's audit (01 owns it). |
| 03 | idempotency | Desktop .tmp-wt-phase4 activation files + tests; Nano: admission_matrix.rs, activation_effects.rs (test rows), trigger_idempotency.rs | Desktop/Nano separated by repo + PR order (Nano first). admission_matrix shared with 04 — 03 lands first in Wave 2 (04 depends_on 03). |
| 04 | controls | admission.rs, receipt.rs, escalation.rs, run_retention.rs, authority.rs, admission_matrix.rs (control rows), controls_bounded.rs, trigger_controls.rs | Sequential after 03; disjoint new modules for escalation/retention. |
| 05 | routines | run_records.rs, effect_cmds.rs (runs listing extension), triggered_runs.rs, session_binding.rs | effect_cmds.rs extends 02's merged file (Wave 1 → Wave 3 separation). |
| 06 | dogfood | scripts/soak/dogfood*.{mjs}, test-dogfood.mjs, phase2_fixture.rs (provisioning extension only), docs/evidence/phase4/dogfood-report.md | Disjoint; consumes 03/05 summaries. |
| 07 | closure | docs/evidence/phase4/{phase4-closure-manifest.json, owner-decision-package.md}, 04-VERIFICATION.md, ledgers, external receipt | Verifier-owned artifact; ledgers flip only on PASS. |

## Three-Strikes and Handoff

Attempts are counted per identical failing assertion/row, derivation property, control ordering, escalation window, retention row, matrix row/seed, CI leg, or verifier query. Before attempt 2 record one root-cause sentence. Before attempt 3 produce an isolating reproduction varying one factor. After strike 3 stop; `.continue-here.md` records exact worktree path/branch/base/head (both repos when cross-repo), dirty diff, command, failing row/seed, CI/PR/run IDs, hypotheses/proof, the exact next command AND its expected output, and prohibited retries. No extra diagnostic machinery is added after strike 3.

## Execution Time Discipline

1. CI is a confirmation, never a debugger. Local gates green before any push. A CI-only failure's next action is a minimal LOCAL reproduction of the failing predicate; a second CI push without a local repro is a rule violation.
2. Round-trip test before integration test: every new wire format / ledger record / state machine gets a serialize→parse (or apply→replay) round-trip unit test in the SAME commit as the type.
3. Cheapest distinguishing experiment: hypothesis sentence before attempt 2, and attempt 2 must be the smallest experiment discriminating the live hypotheses — never a full-pipeline rerun. Full suites run once per plan at the end, focused tests per task.
4. Per-plan wall-time tripwire: each plan carries a `wall_time_tripwire` frontmatter line — 04-01: "2 working days", 04-02: "2 working days", 04-03: "2 working days", 04-04: "3 working days", 04-05: "2 working days", 04-06: "3 working days", 04-07: "1 working day" — plus a 45-minute diagnosis time-box per failure class, then handoff. Hitting the tripwire = stop + .continue-here.md = SUCCESS, not failure.
5. Machine-checkable handoffs: every .continue-here.md entry carries the exact next command AND its expected output, so a resumer verifies state with one command instead of re-deriving context.

## Completion Bar

All of: scheduler retired with sole-fire structural absence green and Cron* replay intact under the untouched frozen journal contract; jobs.json and every touched artifact dispositioned with evidence; typed unknown_outcome on receipts with the manual reconciliation surface and zero exact-once claims; deterministic occurrence keys with admission/receipt/effect dedup proofs and terminal conflict semantics; bounded retry/retention/escalation/emergency-refusal enforced with typed receipts and escalation riding unfrozen ledgers; tombstone sweep decided on record; triggered-run records rebuild-equivalent with scoped recall proven (or the Phase 3 gate honestly recorded); multi-seed dogfood report with full negative matrix; the owner accept/revise/reject recorded from evidence; exact-artifact identity exercised through both Desktop ACP stacks; 7-leg CI and both repositories' governance green; disclosed protected merges with tree/diff/patch identity; and an independent fresh-checkout verifier PASS flipping REQUIREMENTS/ROADMAP/STATE to v1.1 milestone-complete. No new milestone is started.
