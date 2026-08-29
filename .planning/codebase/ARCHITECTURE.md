<!-- refreshed: 2026-08-27 -->
# Architecture

**Analysis Date:** 2026-08-27

## System Overview

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ Entry adapters                                                           │
│ `crates/nano-cli`   `crates/nano-tui`   `crates/nano-protocol`           │
└──────────────────────────────────┬───────────────────────────────────────┘
                                   ▼
┌──────────────────────────────────────────────────────────────────────────┐
│ Agent orchestration                                                      │
│ `crates/nano-agent` — bootstrap, turn loop, tools, approval, goals/tasks │
└───────────────┬──────────────────┬─────────────────────┬─────────────────┘
                ▼                  ▼                     ▼
┌──────────────────────┐ ┌────────────────────┐ ┌──────────────────────────┐
│ Model / network      │ │ Execution boundary │ │ Durable state            │
│ `nano-model`         │ │ `nano-tools`       │ │ `nano-session`           │
│ `nano-egress`        │ │ `nano-sandbox`     │ │ `nano-checkpoints`       │
└──────────────────────┘ │ `nano-cua`         │ │ `nano-memory`            │
                         └────────────────────┘ └──────────────────────────┘
        ┌───────────────────────────┴──────────────────────────────────┐
        ▼                                                              ▼
┌──────────────────────────────┐                       ┌────────────────────┐
│ Extensions                   │                       │ Evidence/contracts │
│ MCP, skills, plugins, hooks │                       │ verify/gates       │
└──────────────────────────────┘                       └────────────────────┘
```

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| CLI adapters | Dispatch doctor, ACP, protocol-host, exec, session, goal, plugin, and verify commands | `crates/nano-cli/src/main.rs` |
| Exec host | Assemble one-shot sessions, ownership, routing, tools, checkpoints, goals, and output | `crates/nano-cli/src/exec_run.rs` |
| ACP host | Maintain interactive ACP sessions, approvals, model selection, and event transport | `crates/nano-cli/src/acp_mode.rs` |
| Agent engine | Run model/tool turns with protection, cancellation, approvals, streaming, hooks, and compaction | `crates/nano-agent/src/turn.rs` |
| Bootstrap | Provide shared session bootstrap and two-layer single-writer exclusion | `crates/nano-agent/src/bootstrap.rs` |
| Tool wiring | Advertise and dispatch filesystem, shell, web, MCP, checkpoint, task, and CUA operations | `crates/nano-agent/src/wiring.rs` |
| Durable journal | Define append-only typed operations and replay state | `crates/nano-session/src/op.rs`, `crates/nano-session/src/replay.rs` |
| Core memory | Persist project/agent-partitioned facts, decisions, episodes, and procedures | `crates/nano-memory/src/store.rs` |
| Memory schema | Own bi-temporal tables, retention, empty KG schema, FTS5, and sqlite-vec | `crates/nano-memory/src/schema.rs` |
| Model boundary | Normalize provider requests, responses, catalogs, retry, and usage | `crates/nano-model/src/lib.rs` |
| Egress boundary | Enforce grants, deny-by-default policy, redaction, and HTTP transport | `crates/nano-egress/src/client.rs` |
| OS containment | Create and supervise platform-native sandboxed processes | `crates/nano-sandbox/src/lib.rs` |
| Tool policy | Apply filesystem, shell, search, image, PTY, and web policy | `crates/nano-tools/src/lib.rs` |
| Extensions | Implement MCP, skills, plugins, hooks, checkpoints, and CUA behind boundaries | `crates/nano-mcp/src/lib.rs`, `crates/nano-skills/src/lib.rs` |
| Gate engine | Execute registered gates, classify evidence, and mint receipts | `crates/nano-verify/src/lib.rs` |

## Pattern Overview

**Overall:** Layered ports-and-adapters workspace with an append-only event journal and fail-closed security boundaries.

**Key Characteristics:**
- Keep shared vocabulary in `crates/nano-core`; keep provider behavior in `crates/nano-model` and OS behavior in `crates/nano-platform`/`crates/nano-sandbox`.
- Build host-specific dependency graphs in `crates/nano-cli`; keep `crates/nano-agent` independent of CLI transports.
- Route session mutation through typed `nano_session::Op` envelopes and reconstruct state with `crates/nano-session/src/replay.rs`.
- Route outbound HTTP through `crates/nano-egress`; never create an independent HTTP path in feature crates.
- Treat `nano-mcp`, `nano-skills`, `nano-plugins`, and `nano-hooks` as bounded inputs to host composition.

## Layers

**Adapters and Presentation:**
- Purpose: Convert terminal, ACP, NDJSON, and TUI input into engine calls and render events.
- Location: `crates/nano-cli`, `crates/nano-tui`, `crates/nano-protocol`.
- Contains: Binaries, parsers, host loops, protocol types, event sinks.
- Depends on: Agent orchestration and service crates.
- Used by: Users, editors, desktop hosts, CI harnesses.

**Agent Orchestration:**
- Purpose: Coordinate bootstrap, model turns, tools, approvals, compaction, goals, tasks, cron, and steering.
- Location: `crates/nano-agent`.
- Contains: `TurnEngine`, `SessionGuardRegistry`, task/MCP registries, tool executors.
- Depends on: Core, model, tools, session, extensions, egress, CUA.
- Used by: CLI and protocol adapters.

**Execution and Security:**
- Purpose: Validate and perform effects under policy, containment, and egress controls.
- Location: `crates/nano-tools`, `crates/nano-sandbox`, `crates/nano-egress`, `crates/nano-cua`, `crates/nano-platform`.
- Contains: Executors, platform isolation, HTTP chokepoint, computer-use backends.
- Depends on: `nano-core` vocabulary and platform libraries.
- Used by: Agent wiring, model clients, MCP, hooks, repomap.

**Durability:**
- Purpose: Preserve replayable sessions, workspace checkpoints, and partitioned long-term memory.
- Location: `crates/nano-session`, `crates/nano-checkpoints`, `crates/nano-memory`.
- Contains: JSONL operations/reducer, attachments, git checkpoints, SQLite memory.
- Depends on: Filesystem locking; memory depends on session journal primitives.
- Used by: Agent bootstrap, CLI hosts, plugins, tools, verification.

**Evidence and Contracts:**
- Purpose: Make claims testable through executable gates and frozen vocabulary.
- Location: `crates/nano-verify`, `gates`, `contracts`, `scripts`.
- Contains: Gate registry/cards, receipts, soak/proof harnesses, JSON/Markdown contracts.
- Depends on: Candidate artifacts and external oracles.
- Used by: CI and release evidence collection.

## Data Flow

### Primary Exec Request Path

1. `main` parses `wayland-nano exec` and creates a current-thread Tokio runtime (`crates/nano-cli/src/main.rs:14`).
2. `exec_run::run` resolves credentials, routing, MCP specs, and factories; `run_exec_with` resolves the seed (`crates/nano-cli/src/exec_run.rs:38`).
3. `bootstrap_session` creates or resumes JSONL; `SessionGuardRegistry` holds lifetime OS ownership plus an in-process mutex (`crates/nano-agent/src/bootstrap.rs:54`).
4. `JournalCoordinator` becomes the append path; replayed envelopes become model context (`crates/nano-cli/src/exec_run.rs:145`).
5. `run_exec_turn` assembles `TurnEngine`, sends requests through `ModelDriver`, dispatches approved `ToolExecutor` calls, and streams events (`crates/nano-cli/src/exec_mode.rs:537`).
6. The host journals outcomes and emits JSONL plus the pinned process exit code (`crates/nano-cli/src/exec_run.rs`).

### Session Replay Path

1. `JournalWriter` appends a versioned `OpEnvelope` as one JSON line (`crates/nano-session/src/writer.rs`).
2. `read_journal` tolerates only a torn final line and rejects malformed middle records (`crates/nano-session/src/reader.rs`).
3. The reducer folds known operations; unknown operation names remain compatible (`crates/nano-session/src/replay.rs`).
4. Bootstrap normalizes interrupted running phases to safe resumable states (`crates/nano-agent/src/bootstrap.rs`).

### Core Memory Write and Recovery Path

1. A host opens `MemoryStore` with a DB, journal path, and `MemoryPolicy` (`crates/nano-memory/src/store.rs:18`).
2. Store methods validate policy, `(project, agent_id)`, identity, trust, and contradiction outcome (`crates/nano-memory/src/store.rs:63`).
3. The store sync-appends `Op::MemoryWriteFact|Decision|Episode|Procedure` before SQLite mutation; mediated writes append `MemoryWriteReceipt` (`crates/nano-memory/src/store.rs:108`).
4. SQLite updates canonical tables plus FTS5 and sqlite-vec; retention keys include project and agent (`crates/nano-memory/src/schema.rs`).
5. `rebuild_from_journals` replays MemoryWrite operations; the session reducer treats them as replay-neutral (`crates/nano-memory/src/store.rs:651`, `crates/nano-session/src/replay.rs:639`).

### Memory Retrieval Path

1. `RetrieveQuery` carries project and `AgentScope`; no global read scope exists (`crates/nano-memory/src/types.rs`).
2. FTS5 BM25 and per-agent sqlite-vec KNN produce bounded candidates (`crates/nano-memory/src/store.rs:529`).
3. Reciprocal-rank fusion combines passes and applies source-tier weights (`crates/nano-memory/src/store.rs`).

**State Management:**
- Session authority is append-only JSONL under `NANO_HOME/sessions`; in-memory state is a replay projection.
- Attachments are content-addressed blobs owned by `crates/nano-session/src/attachment_store.rs`.
- Core memory is a rebuildable SQLite projection whose mutation record is the `MemoryWrite*` journal family.
- `crates/nano-agent/src/memory.rs` is a separate session-local JSON memory surface; `nano-agent` and `nano-cli` manifests contain no `nano-memory` dependency.

## Key Abstractions

**`TurnEngine`:**
- Purpose: Run one bounded model/tool loop with approvals, cancellation, hooks, streaming, and robustness checks.
- Examples: `crates/nano-agent/src/turn.rs`, `crates/nano-agent/src/tasks.rs`.
- Pattern: Dependency injection through `ModelDriver`, `ToolExecutor`, and `ApprovalGate` traits.

**`Op` / `OpEnvelope`:**
- Purpose: Define durable additive session transitions and receipts.
- Examples: `crates/nano-session/src/op.rs`, `contracts/event-types.json`.
- Pattern: Tagged serde enum inside a versioned envelope, reduced into `SessionState`.

**`JournalCoordinator`:**
- Purpose: Provide the single append/compaction coordination point.
- Examples: `crates/nano-session/src/coordinator.rs`, `crates/nano-cli/src/exec_run.rs`.
- Pattern: Shared coordinator behind `Arc`, paired with `SessionGuard` ownership.

**`EgressClient`:**
- Purpose: Centralize network authorization, redirects, transport, and redaction.
- Examples: `crates/nano-egress/src/client.rs`, `crates/nano-model/src/flux_common.rs`.
- Pattern: Explicit endpoint grants and deny-by-default policy.

**`MemoryStore`:**
- Purpose: Mediate journal-first writes and scoped hybrid retrieval.
- Examples: `crates/nano-memory/src/store.rs`, `crates/nano-memory/src/mediation.rs`.
- Pattern: Single-writer SQLite projection with deterministic local embeddings and journal rebuild.

## Entry Points

**Wayland Nano CLI:**
- Location: `crates/nano-cli/src/main.rs`.
- Triggers: `doctor`, `protocol-host`, `acp-host`, `auth`, `exec`, `session`, `goal`, `plugin`, `verify`.
- Responsibilities: Parse arguments, establish runtime/home/workspace, delegate to library orchestration.

**ACP Profile Helper:**
- Location: `crates/nano-cli/src/bin/acp_profile/main.rs`.
- Triggers: ACP profile/integration harness execution.
- Responsibilities: Spawn and exercise ACP child sessions.

**TUI:**
- Location: `crates/nano-tui/src/main.rs`.
- Triggers: Interactive terminal client invocation.
- Responsibilities: Connect to host, compose input, render transcript/modal/status state.

**MCP Fake Server:**
- Location: `crates/nano-mcp/src/bin/wayland-nano-mcp-fake-server/main.rs`.
- Triggers: MCP tests and proof harnesses.
- Responsibilities: Provide deterministic protocol fixtures.

## Architectural Constraints

- **Threading:** CLI hosts use a current-thread Tokio runtime; session mutation is single-writer per journal across processes.
- **Global state:** Session guard registry lives in `crates/nano-agent/src/bootstrap.rs`; credential redaction registry lives in `crates/nano-egress/src/redact.rs`.
- **Network:** All outbound HTTP must pass through `nano-egress`.
- **OS boundary:** Keep OS-specific logic out of the agent loop; use `nano-platform`, `nano-sandbox`, or a CUA backend.
- **Journal:** Add operations without changing old wire tags; unknown operations remain skippable and replay-neutral where appropriate.
- **Subagents:** Keep helpers temporary and bounded to fan-out four and depth one in `crates/nano-agent/src/tasks.rs`.
- **Memory scope:** Core memory permits project reads with explicit agent scopes only; do not add a cross-project/global path.
- **Vendoring:** Do not modify `vendor/`; adapted donor code requires a file-specific `UPSTREAM.md` entry.

## Anti-Patterns

### Adapter-Owned Orchestration

**What happens:** Session lifecycle, replay, or exclusion logic is implemented in a CLI/ACP handler.
**Why it's wrong:** Entry points diverge from the shared bootstrap path.
**Do this instead:** Add lifecycle behavior to `crates/nano-agent/src/bootstrap.rs` or another engine module and keep `crates/nano-cli/src/main.rs` thin.

### Side-Channel Effects

**What happens:** A feature creates its own HTTP client, bypasses tool policy, or mutates state before journaling.
**Why it's wrong:** It defeats egress, containment, replay, and crash durability.
**Do this instead:** Use `crates/nano-egress/src/client.rs`, `crates/nano-tools`, and journal via `crates/nano-session/src/coordinator.rs` before projection mutation.

### Treating Core Memory as Runtime-Wired

**What happens:** Runtime behavior assumes `crates/nano-memory` already participates in agent/CLI composition.
**Why it's wrong:** `crates/nano-agent/Cargo.toml` and `crates/nano-cli/Cargo.toml` contain no `nano-memory`; `crates/nano-agent/src/memory.rs` is distinct.
**Do this instead:** Plan explicit identity, policy, recall-context, mediation, and receipt seams while preserving journal-first storage.

### Graph Logic in Core Memory

**What happens:** Retrieval populates or traverses `kg_nodes`/`kg_edges`.
**Why it's wrong:** `crates/nano-memory/src/schema.rs` is storage-only and retrieval is FTS/KNN/RRF.
**Do this instead:** Keep graph algorithms in a separately contracted package behind an explicit boundary.

## Error Handling

**Strategy:** Typed errors at libraries, fail-closed security decisions, sanitized messages at transports, and stable process/protocol codes.

**Patterns:**
- Define domain errors with `thiserror`, such as `MemoryError` in `crates/nano-memory/src/types.rs` and `GuardError` in `crates/nano-agent/src/bootstrap.rs`.
- Use typed sandbox/busy/usage outcomes instead of fallback behavior.
- Sanitize provider text with `crates/nano-egress/src/redact.rs` before display or journaling.
- Journal interrupted lifecycle state and reconcile it during bootstrap.

## Cross-Cutting Concerns

**Logging:** CLI adapters write bounded diagnostics to stderr; protocol/exec paths emit structured events; durable facts belong in journals and receipts.

**Validation:** Validate CLI input, protocol frames, tool schemas, catalogs, endpoint grants, journal envelopes, memory partitions, and gate inventories at boundaries.

**Authentication:** Provider keys resolve in `crates/nano-cli/src/flux_key.rs` and `crates/nano-cli/src/provider_key.rs`; MCP OAuth lives in `crates/nano-mcp/src/oauth`.

---

*Architecture analysis: 2026-08-27*
