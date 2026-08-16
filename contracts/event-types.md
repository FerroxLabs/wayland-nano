# Wayland Nano — Event Types: the Nano Profile of `wayland-desktop-core` v1

**FROZEN v1.0 — 2026-08-11**
Change control: changes require owner sign-off plus an evidence update (the
corpus replay harness and this document change in the same commit).
Descriptive-first: the sets below are exactly what the conformance harness
enforces. Anchors SCORECARD C3.2.

Corpus: `wayland-desktop-core/v1`, **110 fixtures** (11 commands / 39 events
/ 23 compat / 37 adversarial — counts pinned in
`wayland-nano/crates/nano-protocol/corpus/wayland-desktop-core/v1/manifest.json`
and asserted by the harness). Vendored at
`wayland-nano/crates/nano-protocol/corpus/wayland-desktop-core/v1/` (immutable
upstream copy: `resources/upstreams/wayland-desktop/contracts/wayland-desktop-core/v1`,
pinned producer commit `d0aa0abc…` per
`shared/fixtures/desktop-core-v1/POINTER.md`).

Replay harness: `wayland-nano/crates/nano-protocol/src/corpus.rs`
(`corpus::tests::full_corpus_conformance`) — **fails, never skips, if the
corpus is missing**. Last reported replay (commit `7e4b3bd`): 21 accepted,
5 tolerated, 24 rejected-closed, 90 adversarial/compat handled, 0 violations,
0 panics (COMP-CORPUS-001…005).

## 1. Commands (host → engine) — 11 in corpus

**Accepted (6)** — parse and execute (`messages.rs:20-40`, `corpus.rs:28-35`):
`message`, `stop`, `ping`, `tool_approve`, `tool_deny`, `approval_resume`.
(Plus `shutdown`, handled by the host loop with a typed shutdown —
COMP-PROTO-004.)

**Tolerated-unsupported (5)** — fail as a **typed, recoverable error frame**
and the engine continues; they are **never executed**:
`init_history`, `set_mode`, `set_config`, `add_mcp_server`,
`host_send_message_result`. Rationale: v1 is single-mode (`default`), Nano
owns its own MCP configuration, and session history comes from the journal
(via `session/load`), not host push.

## 2. Events (engine → host) — 39 in corpus

**Accepted (15)** — parse into the `Event` enum and are the only events Nano
emits (`messages.rs:45-124`, `corpus.rs:39-55`):
`ready`, `stream_start`, `text_delta`, `thinking`, `tool_request`,
`tool_running`, `tool_result`, `tool_cancelled`, `approval_required`,
`suspend`, `approval_resume`, `info`, `error`, `stream_end`, `pong`.
Emitted frames are asserted to match corpus shapes
(`messages::tests::emitted_frames_match_corpus_shapes` — COMP-PROTO-002).

**Rejected-closed (24)** — fail typed, never misrouted, never panicked:
`anvil_receipt`, `anvil_receipt_invalidated`, `browser_event`,
`browser_policy_denied`, `budget_exceeded`, `config_changed`, `cua_event`,
`cua_policy_denied`, `evolution_event`, `execution_policy`,
`host_send_message_request`, `mcp_failed`, `mcp_ready`, `plugin_event`,
`plugin_registration_failed`, `provider_circuit_event`, `session_cost`,
`sub_agent_event`, `tool_chunk`, `tool_panicked`, `trace_event`,
`workflow_finished`, `workflow_node_event`, `workflow_started`.
These are the Anvil/workflow, browser, computer-use (CUA), plugin,
sub-agent, and host-orchestration families — v1 non-goals per
`capability-profile.md` §5.

## 3. Typing rules (normative)

1. **Fail closed on unknown types.** Any event or command type outside the
   accepted sets deserializes to a typed error (`Malformed` / host error
   frame), never a panic, never a misroute (`messages.rs:4-6`;
   COMP-CORPUS-002/004).
2. **Forward-additive tolerance on known types.** Unknown *extra fields* on
   a supported shape are tolerated (COMP-CORPUS-002) — a newer producer may
   extend known shapes, but may not introduce new types without a profile
   revision.
3. **No partial dispatch.** A frame either parses fully into a profile type
   or is an error; there is no "parse the type tag and ignore the body"
   path.
4. **Wire-level tolerance, type-level strictness.** The NDJSON codec
   tolerates CRLF, holds partial tails, and survives mixed valid/malformed
   streams (malformed line → error frame, engine continues) —
   COMP-PROTO-003/004.

## 4. Compat and adversarial fixtures

- **Compat (23 files):** every compat fixture is handled — accepted shape or
  typed error — zero panics (COMP-CORPUS-003; count pinned to 23).
- **Adversarial (37 `.jsonl` streams, families: anvil / commands / events /
  policy / workflow):** every line is handled — accepted shape, event shape,
  or typed error — zero panics; fail-closed on unknown-critical-extension,
  stale-replay, sequence-gap, version-mismatch, and friends
  (COMP-CORPUS-004; count pinned to 37, ≥90 handled lines asserted).
- Deferred by the corpus producer (not Nano gaps; listed in
  `manifest.json: deferred_adversarial`): `ordinary_turn_tool_replay_reducer`,
  `anvil_desktop_replay_reducer`, `anvil_persistent_mutation_watcher`.
- Track-B-owned adversarial coverage beyond the corpus: 31 tests in
  `wayland-nano/crates/*/tests/adversarial_{egress,fs,shell,sse}.rs` plus the
  journal fuzz suite (`nano-session/tests/adversarial_journal.rs`) and
  `nano-core/tests/adversarial_policy.rs` — catalog gap G-ADV-1 (closed);
  found and fixed 6 real holes (commit `6e44921`).

## 5. Profile honesty

The capabilities advertised in the `ready` frame match this event surface
exactly: orchestration families rejected here are `false` in the capability
profile, and `mcp`/`skills` flipped true only after live end-to-end proof
(`profile::tests::v1_profile_is_honest_in_corpus_shape` — COMP-CORPUS-005).

## Machine-readable authority

`contracts/event-types.json` is the canonical machine-readable sibling.
Generated by `gen_contracts` from the vendored `wayland-desktop-core/v1` manifest and corpus layout; changes follow the frozen change-control rule above.

