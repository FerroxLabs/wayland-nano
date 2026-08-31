# Phase 4 Context: Bounded Trigger and Immediate Dogfood

**Authority:** ROADMAP Phase 4, REQ-RUN-01, REQ-DOG-01, MEMORY-CONTRACT v1.2, WORKABLE-AGENT-AUTHORITY-AMENDMENT v1.0, Phase 2 verification (PASS), Phase 3 plans (03-01..03-07), Phase 4 research (04-RESEARCH.md). Roadmap formal ordering holds (Phase 4 depends on Phase 3); 04-RESEARCH §7 earmarks which plans are Phase-3-independent if the owner chooses to overlap.

## Goal

Retire Nano scheduling, make Desktop the sole firing authority for repeated bounded Nano work through the shared admission gate, and let end-to-end dogfood evidence decide whether the architecture is workable. Stop at the owner accept/revise/reject decision — no new milestones.

## Locked Decisions

- D4-01: Scheduler retirement means removing firing/management authority, NOT the replay vocabulary. `Op::CronCreated/CronFired/CronDeleted` stay replay-readable forever (frozen `contracts/journal-semantics.json` opVocabulary; `activation_legacy_replay.rs` must stay green). Removed: `cron_fire.rs`, the ticker/tick arm, the cronjob tool wrapper branch, gate arms, and the `CronRunner` machinery. `jobs.json` disposition (leave-inert vs migrate) is decided with evidence in the retirement plan — the store is a cache; journals are authoritative.
- D4-02: Sole-fire negative is executable proof that nothing inside Nano can fire scheduled work: no ticker, no tool, no env/config revival path (the Phase 2 const-quarantine becomes structural absence). Desktop alone triggers, through the shared Nano admission gate, from either ACP stack.
- D4-03: Idempotency is occurrence-keyed at the activation tuple. Desktop derives a deterministic `idempotency_key = f(routine_id, occurrence_instant)` and a stable logical activation id (replacing per-process `randomId()` at `waylandNanoActivation.ts:103-104`); Nano dedups at the existing tuple `issuer\0principal\0project\0idempotency_key` (`admission.rs:916-921`), replays stored receipts on same-key/same-payload, and fails `IdempotencyConflict` on changed bytes. Effect-dispatch dedup follows from `effect_id` embedding `activation_id`; no plan may promise effect-level idempotent redispatch across activations (`effect_id` includes `call_id` — model retries are new effects by design).
- D4-04: The intent ledger is completed, not reinvented: `EffectRecord::{Intent,Result,UnknownOutcome}` already exists with durable intent-before-dispatch (`activation_effects.rs:82-198`); Phase 4 surfaces typed `unknown_outcome` to receipts, adds the manual-reconciliation surface (list/resolve pending effects), and audits all wording so no exact-once claim exists. Ambiguity stays terminal-refuse + manual reconciliation.
- D4-05: Bounded execution controls: cancel/pause for triggered runs reuse the signed-control machinery (`admission.rs:566-699`); retry is budget-capped and bounded; escalation is GREENFIELD — a journaled, rate-limited attention mechanism that rides the activation/effects ledgers (outside the frozen session-journal contract) unless the owner explicitly signs a `journal-semantics.json` amendment; an emergency-refusal receipt reason is added; per-routine run-record retention (~50/20 starting points) is modeled on `RetentionCaps` (`nano-memory/types.rs:134-153`); the plan decides whether authority-journal nonce-tombstone sweep is in scope (today nothing prunes them).
- D4-06: Dogfood is evidence-first: an activation-aware harness (extend `scripts/soak` with `phase2-fixture` provisioning — the chassis is currently unauthenticated, this is explicit work) drives repeated interactive + scheduled activations incl. crash-ambiguity, cancel, replay/remap/denial negatives; receipts and a report support the owner's accept/revise/reject decision and next-milestone choice. The report shape reuses 03-05's continuity-report pattern.
- D4-07: Phase 3 coupling is explicit: `memory_recall` carriers and scoped-recall dogfood legs are gated on 03-02/03-03 (until then, triggered runs are fresh/session_resume only); `activation_quarantine.rs` is edited by both 03-04 (memory migration assertions) and 04-01 (cron source-text assertion rewrite) — merge order is a declared hazard, sequenced in the plans.
- D4-08: Anti-scope (stop/replan tripwires): no provider work, no Nano timer/schedule reintroduction, no UI, no unbounded retry, no extraction/graph/KG, no cross-project work, no bot-product UX (Desktop's scope), no teach-by-demonstration, no model-based auto-approval.
- D4-09: Governance, evidence bar, and the Phase 3 wall-time discipline carry over unchanged: CI is confirmation never debugger; round-trip tests before integration tests; cheapest distinguishing experiment; per-plan wall-time tripwires (stop = success); machine-checkable handoffs; separate authorized worktrees; disclosed owner-directed TradeCanyon governance; ledgers flip only on independent-verifier PASS.

## Acceptance

- REQ-RUN-01 and REQ-DOG-01 mapped exactly once to executable plans.
- Scheduler inventory/migration/removal merged with the quarantine regression gate green (rewritten, not deleted) and legacy `Cron*` replay intact.
- Sole-fire negative green; deterministic occurrence keys dedup across admission/journal/receipt/effect ID in tests.
- Intent-ledger `unknown_outcome` + manual reconciliation proven by crash-ambiguity rows; no exact-once claim in any surface.
- Bounded retry/retention/escalation/emergency-refusal enforced in code with typed receipts.
- Dogfood receipts + report; owner decision recorded; exact-artifact identity exercised through both Desktop ACP stacks; both CI systems green; independent phase verification PASS flips the ledgers.

## Discipline

- `contracts/journal-semantics.json` is frozen under owner sign-off — any vocabulary amendment is an owner gate, never an executor decision.
- `gates/**`/`agents/**` §6.6 owner review unchanged. No `.secrets` reads. Three strikes; exact handoff on stop.
- Desktop edits only in an authorized Desktop worktree on owned paths (Phase 2 D2-13/D2-19 pattern); Nano agents never edit Desktop without it.
