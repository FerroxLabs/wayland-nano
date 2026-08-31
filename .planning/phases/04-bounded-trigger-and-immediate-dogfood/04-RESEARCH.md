# Phase 4 Research: Bounded Trigger and Immediate Dogfood

**Method:** independent reconnaissance agent over fresh detached checkouts of merged `origin/master` (`c10dcb9`) and Desktop `origin/main` (`0b7f029d`), all claims file:line-cited; condensed here. Nothing was modified.

## 1. Cron inventory — what retirement touches

All cron lives in two files plus journal vocabulary and gate pins:

- `crates/nano-agent/src/cron.rs` (1137 lines, fully preserved legacy): 5-field parser `parse_schedule` (`:67-111`), `occurrence_id()` `{job_id}:{rfc3339-minute}` (`:255-259`), injection scan `scan_cron_prompt` (`:292-329`), `JsonCronStore` at `<nano_home>/cron/jobs.json` with atomic 0600 save (`:386-444`), `CronRunner.tick`/`tick_one` with coalesce-to-one, occurrence idempotency, claim-before-fire, journal-first `Op::CronFired` reservation (`:559-877`), `cronjob_tool_definition()` (`:886-905`), `CronjobExecutor::quarantined` forced-call denial (`:945-992`).
- `crates/nano-cli/src/cron_fire.rs` (256 lines): `const fn phase2_cron_quarantined() -> bool { true }` (`:20-22`); everything below the guards is unreachable legacy.
- Wiring/quarantine points: `acp_mode.rs:4598-4614` (tool wrapper branch), `:1958` + `:2060-2064` (ticker only when unquarantined), `:5817-5863` (tick arm with fail-closed latch), `:6530-6540` (gate arm) + matrix test `:8666-8756`; `exec_run.rs:309-322` (always quarantined); `exec_mode.rs:273-280` (typed deny every mode).
- Journal: `Op::CronFired/CronCreated/CronDeleted` (`op.rs:1474-1509`), frozen in `contracts/journal-semantics.json` opVocabulary (`changeControl: "owner sign-off + evidence update"`); replay folds `replay.rs:191-208, 517-562`; legacy-replay guarantee `nano-session/tests/activation_legacy_replay.rs:9-73`.
- No config reader for cron; `NANO_CRON_ENABLED` exists only as a deliberately-ignored canary in the quarantine test (`activation_quarantine.rs:259`).
- Regression gate: `activation_quarantine.rs` — including source-text production-wiring assertions (`:151-166`) that must be rewritten in lockstep with any removal.
- Full removal touch-list: `cron.rs` (delete machinery, keep nothing executable), `cron_fire.rs` + its `lib.rs:9` export, `acp_mode.rs` (ticker, tick arm, wrapper branch, gate arm + matrix test), `exec_run.rs`, `exec_mode.rs` + `exec_tests.rs:481-494`, `jobs.json` disposition decision, `UPSTREAM.md:148` provenance, `docs/SEVERITY-MAP.md:31,54,94,128-129`, `cron_tests.rs`, `fork_tests.rs:443-627`, `scripts/s6-proof/`. `Op::Cron*` variants and replay folds STAY.

## 2. Effect dispatch — the intent ledger already exists

- Direct path `crates/nano-agent/src/activation_effects.rs`: per call — capability map → policy check → live authority revalidation (`validate_live_effect_authority :120-130`) → `effect_id = sha256({activation_id, arguments, call_id, tool})` (`:233-236`) → ledger append `activation/effects.jsonl`.
- `EffectRecord::{Intent,Result,UnknownOutcome}` (`:82-97`): terminal ⇒ refuse; pending intent ⇒ append UnknownOutcome + typed "reconciliation required" refusal; intent durable BEFORE dispatch; result after; crash-fault seam `EffectFault::AfterDispatch`. Tests `nano-agent/tests/activation_effects.rs:58-160` (ambiguous effect never redispatches; budget refusal precedes dispatch).
- Delegated path (MCP + task spawn) `nano-agent/src/mcp.rs`: `DelegatedEffectAuthority.begin()` same ledger/semantics; strict-subset budget/capability delegation for task spawn; `recheck_live_authority` re-reads `admission.jsonl`.
- Activation-level `record_unknown_outcome` (`admission.rs:539-552`), `ResultState::UnknownOutcome` (`receipt.rs:46`), crash-rebuild proof (`replay_crash.rs:20-51`).
- Retry: none automatic; ambiguity is terminal + manual. `effect_id` embeds `call_id` → occurrence-level dedup must come from the activation binding, not the effect ledger.

## 3. Admission gate — what Phase 4 reuses free

- Idempotency: global nonce consume + expiry tombstone; tuple key `issuer\0principal\0project\0idempotency_key` (`admission.rs:916-921`); same-key/same-payload replays stored receipt (`:309-315`); changed bytes ⇒ `IdempotencyConflict`; journal-first Intent→Decision; crash-safe ledger with sequence-gap rejection and torn-tail truncate; control-nonce dedup.
- Budgets/deadline: carrier fields (`lib.rs:246-247,305-312`), closed min-cap intersection (`policy.rs:84-134`), deadline re-checks at session bind and every effect.
- Cancel/pause: signed controls with RaceLost vs terminal semantics, receipts per control (`admission.rs:566-699`).
- **Escalation: confirmed ABSENT** — no `Op::AttentionRequested`, no attention mechanism, no `RejectReason` variant. Greenfield vocabulary; the frozen `journal-semantics.json` makes a session-journal op an owner-sign-off amendment — prefer the activation/effects ledgers (outside that contract).
- Emergency refusal: signed refusal receipts + kill switches exist (`DisableArtifact`, `RevokeKey`/`RevokeIssuer`/`RetireSubject`); no dedicated "emergency" receipt reason yet.

## 4. Desktop trigger seam

- Carrier fields a recurring trigger populates (`desktop .../activation/types.ts:44-55`): logicalActivationId, sessionId, continuity, capabilities, budgets, deadline, controls, time bounds; signing adds `nonce` + `idempotency_key` — **both currently `randomId()`** (`waylandNanoActivation.ts:103-104`).
- `#retryAssertions` keyed by logicalActivationId returns the same signed assertion on in-process retry; changed inputs ⇒ `WaylandNanoActivationRetryConflictError`. Process-scoped only — a Desktop restart mints a fresh key and would NOT dedup at Nano.
- Deterministic derivation binds to: `idempotency_key = f(routine_id, occurrence_instant)` + stable logicalActivationId (today `operation\0sessionId`, `waylandNanoActivationOwner.ts:190-197`). Effect dedup follows via `effect_id ⊇ activation_id` + admission receipt replay.
- Continuity selection today: fresh/session_resume only (`waylandNanoActivationOwner.ts:262-275`) — `memory_recall` waits on Phase 3 D3-04.
- Desktop-side design note: deterministic occurrence keys change `WaylandNanoActivationRetryConflictError` semantics (retry map assumes random-per-logical-id) — flagged for the Desktop plan, not solved here.

## 5. Retention/caps

- `RetentionCaps { episodes, facts, bytes }` (`nano-memory/types.rs:134-153`) + `retention_control` table + ordered enforcement deletes (`store.rs:529-548`) — the pattern for per-routine run-record caps (~50/20).
- Activation nonce tombstones carry `expires_at_unix` (`authority.rs:34-37`) but **no sweep exists** — tombstones accumulate; decide ownership in the controls plan.
- `admission.jsonl`/`effects.jsonl` are unbounded append-only; the cron store's "cache rebuilt from authoritative journal" doctrine (`cron.rs:5-10`) is the template for reconstructible run records.

## 6. Dogfood harness substrate

- `scripts/soak/soak.mjs`: seeded ACP driver + budget evaluation + journal verifier — but **unauthenticated**; dogfood needs an activation-aware mode (explicit work, not reuse-as-is).
- `crates/nano-activation/src/phase2_fixture.rs` provisions a complete frozen authority (admin/issuers/keys/enablement) with CLI entry points — the repeatable provisioning base for dogfood homes.
- Phase 3's planned `scripts/soak/continuity.mjs` + report shape (03-05) is the evidence-chassis template.
- Existing batteries to extend: `nano-activation/tests/{admission_matrix,replay_crash,...}`, `nano-agent/tests/{activation_effects,activation_delegated_effects}`, Desktop `verify-wayland-nano-activation.ts` matrix.

## 7. Phase 3 dependency map

**Depends on Phase 3:** scoped-recall dogfood legs (need 03-02/03-03); `memory_recall` carriers (03-02 lifts `ContinuityNotEnabled` at `admission.rs:891-893`); `activation_quarantine.rs` is edited by BOTH 03-04 (memory) and 04-01 (cron assertions) — merge-order hazard, sequence explicitly.

**Phase-3-independent (can overlap if the owner chooses):** cron retirement (04-01); intent-ledger completion + reconciliation (04-02); occurrence-idempotency derivation + dedup proofs (04-03); bounded controls/escalation/retention (04-04); interactive fresh/session_resume dogfood legs.

## Risks for the planner

1. Frozen journal contract: session-journal vocabulary additions need owner sign-off; prefer activation/effects ledgers.
2. `activation_quarantine.rs` source-text assertions break by design on cron removal — rewrite in the same plan; coordinate with 03-04.
3. `Op::Cron*` must stay replay-readable — retirement removes authority, not the reader.
4. No effect-level idempotent redispatch promises: `effect_id` embeds `call_id`.
5. Tombstone sweep is unsized — decide scope explicitly in 04-04.
6. Soak chassis is unauthenticated — activation-aware dogfood driver is new work, unproven at soak scale.
7. Escalation is greenfield mechanism + vocabulary, not an extension — size accordingly.
