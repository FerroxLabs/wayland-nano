<!-- refreshed: 2026-08-16 -->
# Architecture

**Analysis Date:** 2026-08-16

## System Overview

```text
┌─────────────────────────────────────────────────────────────┐
│                    Host / Product Layer                     │
├──────────────────┬──────────────────┬───────────────────────┤
│ CLI + ACP/NDJSON │ Terminal UI      │ Packaging / proofs    │
│ `crates/nano-cli`│ `crates/nano-tui`│ `packaging/`, `scripts/`│
└────────┬─────────┴────────┬─────────┴──────────┬────────────┘
         │                  │                     │
         ▼                  ▼                     ▼
┌─────────────────────────────────────────────────────────────┐
│       Agent orchestration and protocol adaptation           │
│ `crates/nano-agent`, `crates/nano-protocol`                 │
├──────────────────┬──────────────────┬───────────────────────┤
│ model + egress   │ tools + sandbox  │ session + extensions  │
│ `nano-model`     │ `nano-tools`     │ `nano-session`        │
│ `nano-egress`    │ `nano-sandbox`   │ MCP/skills/plugins    │
└────────┬─────────┴────────┬─────────┴──────────┬────────────┘
         │                  │                     │
         ▼                  ▼                     ▼
┌─────────────────────────────────────────────────────────────┐
│ OS / network / durable-state boundaries and shared contracts│
│ `crates/nano-platform`, `../shared/contracts/`, `$NANO_HOME`│
└─────────────────────────────────────────────────────────────┘
```

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| Product entry points | Dispatch doctor, protocol, ACP, exec, session, goal, rules, and plugin commands | `wayland-nano/crates/nano-cli/src/main.rs` |
| Host composition | Assemble production model, tools, MCP, skills, memory, checkpoints, and journal services | `wayland-nano/crates/nano-cli/src/host_mode.rs` |
| ACP adapter | Translate Desktop ACP lifecycle and streamed turns into the same runtime services | `wayland-nano/crates/nano-cli/src/acp_mode.rs` |
| Agent runtime | Own the turn state machine, budgets, loop protection, bounded tasks, steering, goals, and tool routing | `wayland-nano/crates/nano-agent/src/turn.rs` |
| Model boundary | Expose provider-neutral requests/events and provider clients behind drivers | `wayland-nano/crates/nano-model/src/types.rs` |
| Policy boundary | Define permission profiles, policy evaluation, sensitive paths, and execution rules | `wayland-nano/crates/nano-core/src/permissions.rs` |
| Tool boundary | Enforce filesystem/search/shell/web operations; shell execution delegates to containment | `wayland-nano/crates/nano-tools/src/lib.rs` |
| OS containment | Implement Windows, macOS, and Linux process isolation and cleanup | `wayland-nano/crates/nano-sandbox/src/lib.rs` |
| Network containment | Apply deny-by-default egress policies and redaction-safe errors | `wayland-nano/crates/nano-egress/src/lib.rs` |
| Durable state | Append, coordinate, replay, recover, fork, and compact session journals | `wayland-nano/crates/nano-session/src/lib.rs` |
| Wire protocol | Encode/decode NDJSON, publish honest capabilities, and run ready-first host framing | `wayland-nano/crates/nano-protocol/src/host.rs` |
| Extension boundaries | Integrate MCP servers, skills, plugins, hooks, checkpoints, and CUA without expanding core types | `wayland-nano/crates/nano-mcp/src/lib.rs` |
| External contract authority | Define cross-track capabilities, errors, journal semantics, and scorecard evidence | `shared/contracts/`, `shared/SCORECARD.md` |

## Pattern Overview

**Overall:** Layered hexagonal runtime with trait seams and composition roots.

**Key Characteristics:**
- Keep hosts thin: assemble concrete adapters in `wayland-nano/crates/nano-cli/src/host_mode.rs` and `wayland-nano/crates/nano-cli/src/acp_mode.rs`, then drive `nano-agent` abstractions.
- Keep universal agent and model types provider-neutral; place provider-specific wire behavior in `wayland-nano/crates/nano-model/`.
- Enforce security at two independent boundaries: HTTP through `wayland-nano/crates/nano-egress/`, processes through `wayland-nano/crates/nano-sandbox/`.
- Treat `wayland-nano/crates/nano-session/` as the append-only authority for session mutations and use one `JournalCoordinator` per journal.
- Add optional capabilities through wrappers and registries around `ToolExecutor`, not branches embedded in the turn engine.

## Layers

**Host and Presentation:**
- Purpose: Parse product commands and adapt stdio, ACP, headless exec, and terminal interactions.
- Location: `wayland-nano/crates/nano-cli/`, `wayland-nano/crates/nano-tui/`
- Contains: Binary entry points, host loops, argument parsing, session-facing UX, composition roots.
- Depends on: Agent, protocol, model, tools, policy, journal, and extension crates.
- Used by: Wayland Desktop, terminals, proof scripts, and npm-packaged binaries.

**Orchestration:**
- Purpose: Execute a bounded agent turn without owning OS or provider details.
- Location: `wayland-nano/crates/nano-agent/`
- Contains: `ModelDriver`/`ToolExecutor` seams, turn state, loop protection, tasks, goals, memory, review, steering.
- Depends on: `nano-core`, `nano-model`, `nano-tools`, `nano-session`, and extension crates.
- Used by: `nano-cli`, `nano-protocol`, and `nano-plugins`.

**Adapters and Enforcement:**
- Purpose: Convert abstract model/tool operations into provider, filesystem, process, network, and extension actions.
- Location: `wayland-nano/crates/nano-model/`, `wayland-nano/crates/nano-tools/`, `wayland-nano/crates/nano-egress/`, `wayland-nano/crates/nano-sandbox/`, `wayland-nano/crates/nano-mcp/`
- Contains: Flux/provider clients, SSE codecs, tool implementations, sandbox backends, egress policy, MCP transports.
- Depends on: Core policy and external libraries; `nano-tools` delegates shell containment to `nano-sandbox`.
- Used by: Host composition and agent wiring.

**Persistence and Contracts:**
- Purpose: Preserve recoverable history and stabilize cross-process/cross-track interfaces.
- Location: `wayland-nano/crates/nano-session/`, `wayland-nano/crates/nano-protocol/`, `shared/contracts/`
- Contains: Op journal, coordinator, replay, redaction, NDJSON/ACP messages, error and capability contracts.
- Depends on: Provider-neutral model types where journal payloads require them.
- Used by: Every host and verification harness.

## Data Flow

### Primary NDJSON Request Path

1. `main` selects `protocol-host` and creates a current-thread Tokio runtime (`wayland-nano/crates/nano-cli/src/main.rs:31`).
2. `host_mode::run` constructs permission policy, tools, egress/model driver, registries, wrappers, and the journal coordinator (`wayland-nano/crates/nano-cli/src/host_mode.rs:30`).
3. `run_host_loop` emits `ready`, decodes commands, frames each message, and invokes the turn closure (`wayland-nano/crates/nano-protocol/src/host.rs:55`).
4. `TurnEngine` calls the model through `ModelDriver`, routes tool calls through layered `ToolExecutor` wrappers, and applies loop/budget state (`wayland-nano/crates/nano-agent/src/turn.rs:304`).
5. Model HTTP crosses `EgressClient`; shell/process work crosses `ShellTool` and `nano-sandbox`; mutations append through `JournalCoordinator` (`wayland-nano/crates/nano-agent/src/wiring.rs`).
6. Protocol events are encoded and flushed as NDJSON through stream end (`wayland-nano/crates/nano-protocol/src/host.rs:105`).

### ACP / Desktop Flow

1. `main` selects `acp-host` and calls the ACP adapter (`wayland-nano/crates/nano-cli/src/main.rs:17`).
2. The adapter handles initialize, session creation/resume, prompts, cancellation, permission requests, and session journals (`wayland-nano/crates/nano-cli/src/acp_mode.rs`).
3. Prompt handling invokes streaming turn APIs and translates runtime events into ACP updates (`wayland-nano/crates/nano-cli/src/acp_mode.rs:4017`).
4. Session operations remain journal-first through `wayland-nano/crates/nano-session/src/coordinator.rs`.

**State Management:**
- Use append-only JSONL session journals under `NANO_HOME`, coordinated by `JournalCoordinator`; reconstruct state through replay rather than mutable database records (`wayland-nano/crates/nano-session/src/replay.rs`).
- Keep per-turn mutable state in `TurnState` and bounded wrapper-owned cells; shared registries use `Arc<Mutex<_>>` at host composition sites (`wayland-nano/crates/nano-cli/src/host_mode.rs`).

## Key Abstractions

**ModelDriver:**
- Purpose: Isolate the turn engine from provider transport and wire variants.
- Examples: `wayland-nano/crates/nano-agent/src/turn.rs`, `wayland-nano/crates/nano-agent/src/wiring.rs`
- Pattern: Async port with concrete Flux/provider adapters.

**ToolExecutor:**
- Purpose: Provide one routing seam for built-in, MCP, session, checkpoint, memory, and policy-aware tools.
- Examples: `wayland-nano/crates/nano-agent/src/turn.rs`, `wayland-nano/crates/nano-agent/src/mcp_session_tools.rs`
- Pattern: Decorator chain assembled by each host.

**PermissionProfile / PolicyEngine:**
- Purpose: Convert product permission modes into enforceable filesystem and execution decisions.
- Examples: `wayland-nano/crates/nano-core/src/permissions.rs`, `wayland-nano/crates/nano-core/src/policy_engine.rs`
- Pattern: Immutable policy value passed into tool adapters.

**JournalCoordinator:**
- Purpose: Serialize journal-first state transitions and preserve append-only semantics.
- Examples: `wayland-nano/crates/nano-session/src/coordinator.rs`, `shared/contracts/journal-semantics.md`
- Pattern: Session-scoped shared coordinator.

## Entry Points

**Wayland Nano CLI:**
- Location: `wayland-nano/crates/nano-cli/src/main.rs`
- Triggers: `wayland-nano` command invocation.
- Responsibilities: Command dispatch, runtime creation, exit-code mapping.

**Terminal UI:**
- Location: `wayland-nano/crates/nano-tui/src/main.rs`
- Triggers: TUI binary invocation.
- Responsibilities: ACP client presentation, input, transcript, modal, and terminal lifecycle.

**Protocol Host:**
- Location: `wayland-nano/crates/nano-cli/src/host_mode.rs`
- Triggers: `wayland-nano protocol-host`.
- Responsibilities: Production dependency construction and NDJSON host execution.

**ACP Host:**
- Location: `wayland-nano/crates/nano-cli/src/acp_mode.rs`
- Triggers: `wayland-nano acp-host` from Desktop custom-agent registration.
- Responsibilities: ACP lifecycle and streamed runtime translation.

## Architectural Constraints

- **Threading:** Product hosts use a current-thread Tokio runtime; explicitly shared registries and session posture use `Arc<Mutex<_>>` (`wayland-nano/crates/nano-cli/src/main.rs`).
- **Global state:** Avoid application-wide mutable state; test/soak seams may use guarded `OnceLock` state in `wayland-nano/crates/nano-agent/src/wiring.rs`.
- **Circular imports:** Cargo crate dependencies remain directed toward core/adapters; do not introduce reverse dependencies from enforcement crates into `nano-cli`.
- **OS isolation:** Keep target-specific containment in `wayland-nano/crates/nano-sandbox/` and OS abstraction in `wayland-nano/crates/nano-platform/`; the agent loop must not inspect OS details.
- **Egress:** All outbound HTTP must use `wayland-nano/crates/nano-egress/`; workspace linting treats bypasses as architecture violations.
- **Failure posture:** Missing containment or corrupted security/state stores must produce typed failure, never degraded execution.
- **Scope authority:** `wayland-nano/` and `shared/` are active; `nano/` and `resources/upstreams/` are read-only donor context; `wayland-nano/.tmp-wt-*` is never architectural truth (`wayland-nano/AGENTS.md`).

## Anti-Patterns

### Host Logic in the Agent Loop

**What happens:** Protocol, ACP, provider, or OS-specific branches are added to turn-state code.
**Why it's wrong:** It couples orchestration to a host or platform and violates the constitution boundary.
**Do this instead:** Add an adapter in `wayland-nano/crates/nano-cli/`, `wayland-nano/crates/nano-protocol/`, or `wayland-nano/crates/nano-platform/` and satisfy the seams in `wayland-nano/crates/nano-agent/src/turn.rs`.

### Direct Network or Process Execution

**What happens:** A feature constructs an HTTP client or spawns a process outside the enforcement crates.
**Why it's wrong:** It bypasses deny-by-default egress, redaction, containment, and tree cleanup.
**Do this instead:** Route HTTP through `wayland-nano/crates/nano-egress/src/client.rs` and shell/process work through `wayland-nano/crates/nano-tools/src/shell.rs` plus `wayland-nano/crates/nano-sandbox/`.

### Uncoordinated Session Writes

**What happens:** Multiple feature wrappers append or rewrite session state independently.
**Why it's wrong:** Ordering, recovery, and journal-first invariants become unverifiable.
**Do this instead:** Share the session's `JournalCoordinator` from `wayland-nano/crates/nano-session/src/coordinator.rs` and represent changes as operations in `wayland-nano/crates/nano-session/src/op.rs`.

## Error Handling

**Strategy:** Typed, fail-closed errors at boundaries; host adapters map errors to protocol frames or stable exit codes while continuing only for explicitly recoverable input errors.

**Patterns:**
- Malformed NDJSON becomes a typed protocol error frame and the host continues (`wayland-nano/crates/nano-protocol/src/host.rs`).
- Sandbox unavailability, corrupt plugin stores, and security-boundary failures refuse the operation or host startup (`wayland-nano/crates/nano-cli/src/host_mode.rs`).
- Egress errors redact sensitive material before display (`wayland-nano/crates/nano-egress/src/redact.rs`).

## Cross-Cutting Concerns

**Logging:** Host diagnostics use stderr; structured runtime state and proof evidence live in journals and explicit evidence artifacts (`wayland-nano/crates/nano-cli/src/host_mode.rs`, `shared/reviews/`).
**Validation:** Deserialize into typed protocol/config values, validate at adapter boundaries, and enforce paths/permissions again inside tools (`wayland-nano/crates/nano-protocol/src/codec.rs`, `wayland-nano/crates/nano-core/src/policy_engine.rs`).
**Authentication:** Provider/MCP credential resolution stays in CLI adapter modules and is injected into clients; keys do not enter universal types or frames (`wayland-nano/crates/nano-cli/src/flux_key.rs`, `wayland-nano/crates/nano-cli/src/provider_key.rs`).

---

*Architecture analysis: 2026-08-16*
