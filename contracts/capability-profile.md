# Wayland Nano — Capability Profile (v1)

**FROZEN v1.0 — 2026-08-11**
Change control: changes to this document require owner sign-off plus an
evidence update (the proving test/fixture cited on the changed line must be
updated in the same change). Descriptive-first: every line below is proven by
code, tests, or recorded fixtures. If it is not proven, it is not in this
contract. Scenario IDs refer to `wayland-nano/docs/compliance/SCENARIO_CATALOG.md`.

Implementation of record: `wayland-nano/` (Track B, owner-promoted C1/C2/C3,
`wayland-nano/docs/STATUS.md`).

## 1. Identity and wire

- Engine id `nanok3`, protocol version 1, NDJSON wire (one JSON frame per
  line), malformed-tolerant codec (CRLF tolerated, partial tail held, no
  panic on garbage). Evidence: `wayland-nano/crates/nano-protocol/src/messages.rs:12-14`,
  `src/codec.rs` — COMP-PROTO-003.
- ACP (Agent Client Protocol) over stdio JSON-RPC, `protocolVersion: 1`.
  Evidence: `wayland-nano/crates/nano-protocol/src/acp.rs:19` — COMP-PROTO-005.

## 2. ACP surface (what Desktop can call)

Advertised in the `initialize` response (`acp.rs:92-108`,
`acp::tests::initialize_response_shape`):

| Capability | Value | Evidence |
|---|---|---|
| `loadSession` | **true** | `acp.rs:97`; live-proven: full app restart → `session/load` SUCCESS, codeword oracle recalled (SCORECARD C3.1 leg, `shared/reviews/C3/trackb-desktop-live-evidence.md`) |
| `promptCapabilities.text` | true | `acp.rs:98`; COMP-CLI-002/003 (live prompt through Desktop protocol) |
| `promptCapabilities.image` | false | `acp.rs:99` |
| `promptCapabilities.embeddedContext` | false | `acp.rs:100` |

Supported methods: `initialize`, `session/new`, `session/load`,
`session/prompt`, `session/cancel` (notification), `session/update`
(notification), `session/request_permission` (agent→host approval bridge).
Everything else fails typed with JSON-RPC `-32601 method not found`, never
panics (`acp.rs:62-65`, `acp::tests::method_not_found_is_typed`) —
COMP-PROTO-005.

`session/cancel` stops the turn at a step boundary with
`stopReason: "cancelled"` and the session survives — live-proven in Desktop
(C3 evidence, cancel mid-turn) and COMP-AGENT-006.

## 3. Advertised capability flags (Desktop `ready` frame shape)

`wayland-nano/crates/nano-protocol/src/profile.rs:10-31`, asserted by
`profile::tests::v1_profile_is_honest_in_corpus_shape` (COMP-CORPUS-005):

| Flag | Value | Proving evidence |
|---|---|---|
| `cost_attribution` | true | `usage.cost_usd` parsed from recorded fixtures (COMP-MODEL-001) |
| `mcp` | true | live vertical slice: model called `mcp__fake__probe` through the registry (COMP-CLI-003; STATUS "Capabilities flipped after slice proof") |
| `streaming_tools` | true | streamed live turns, per-frame flush (COMP-CLI-003, G-C2-1 cadence test) |
| `structured_traces` | true | NDJSON event frames in corpus shapes (COMP-PROTO-002) |
| `thinking` | true | `reasoning_content` deltas parsed from recorded streaming fixtures (COMP-MODEL-002) |
| `tool_approval` | true | ACP `session/request_permission` round-trip live in Desktop: Allow once → write verified on disk (C3 evidence 07-10) |
| `memory_enabled` | false | no memory subsystem exists |
| `plugins` | false | no plugin subsystem exists |
| `sub_agent_traces` | false | no sub-agent orchestration exists |
| `browser_suite` | false | non-goal (§5) |
| `computer_use` | false | non-goal (§5) |
| `modes` | `["default"]`, `current_mode: "default"` | `profile.rs:27-28` |
| `extensions.skills` | true | live slice: skill instruction (SKILLCONFIRMED) visible in model reply (COMP-CLI-003) |

Honesty rule (normative): a flag flips to true only AFTER end-to-end proof,
never on intent — both `mcp` and `skills` were flipped only after the live
slice proof (`profile.rs:11-14`, STATUS).

## 4. Tools, MCP, skills

- **Model-facing tool surface (v1, complete):** `fs_read`, `fs_write`,
  `fs_edit`, `shell` — `wayland-nano/crates/nano-agent/src/wiring.rs:34-90`
  (`v1_tool_definitions`). fs tools are policy-enforced (workspace-write
  containment, sensitive-file deny, bounded reads — COMP-TOOLS-001); `shell`
  runs only through the sandbox spawn path (COMP-TOOLS-003). Proven
  end-to-end by the live C2 fixture task (read → patch → run tests → verify;
  COMP-AGENT-005).
- **MCP client:** stdio servers (handshake + `tools/list`, initialize
  advertises `nanok3` — COMP-MCP-001) and streamable-HTTP against Flux
  `/mcp/` (SSE-framed and plain-JSON responses both parsed — COMP-MCP-003/004).
  Agent-side routing: namespaced `mcp__<server>__<tool>` calls, unknown
  namespaces rejected (COMP-AGENT-002).
- **Skills:** scoped loader with bounded activation; malformed skills surface
  as errors, never silently dropped; frontmatter repair/sanitize
  (COMP-SKILLS-001/002). Skill context reaches the model only when valid
  skills exist (COMP-AGENT-003).

## 5. Non-goals (what Nano v1 does NOT do)

False in the profile and absent from the implementation; the corresponding
corpus event families fail closed (see `event-types.md`):

- No Anvil (workflow/receipt orchestration), no Mesh/Fleet multi-agent, no
  browser suite, no computer-use, no plugins, no memory subsystem, no
  sub-agent traces, no cron/scheduled execution, no provider mode switching
  (single `default` mode; `set_mode` is a tolerated-unsupported corpus
  command).

These are v1 non-goals, not roadmap items; this contract makes no promise
about them in either direction.

## Machine-readable authority

`contracts/capability-profile.json` is the canonical machine-readable sibling.
Generated by `gen_contracts` from `nano_protocol::profile::v1_capabilities()`; changes follow the frozen change-control rule above.

