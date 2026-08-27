# Codebase Structure

**Analysis Date:** 2026-08-27

## Directory Layout

```text
wayland-nano/
├── .github/workflows/      # CI gates and release automation
├── .planning/codebase/     # Ferrox current-state codebase maps
├── contracts/              # Paired Markdown/JSON public contracts
├── crates/                 # Rust workspace crates
│   ├── nano-cli/           # Product binaries and host composition
│   ├── nano-agent/         # Turn loop and orchestration
│   ├── nano-core/          # Shared provider/OS-neutral vocabulary
│   ├── nano-session/       # Append-only journal and replay
│   ├── nano-memory/        # Project/agent-scoped SQLite memory
│   ├── nano-model/         # Provider/model boundary
│   ├── nano-tools/         # Policy-enforced effects
│   ├── nano-sandbox/       # OS process containment
│   ├── nano-egress/        # Outbound HTTP chokepoint
│   └── ...                 # Extensions, UI, CUA, verification
├── docs/                   # Status, compatibility, compliance, evidence
├── gates/                  # Gate cards, registry, fixtures, validators
├── packaging/npm/          # npm launcher/install packaging
├── scripts/                # Proof, soak, provisioning, evidence harnesses
├── vendor/                 # Immutable pinned donor trees
├── AGENTS.md               # Repository-wide agent rules
├── ARCHITECTURE.md         # Architecture constitution
├── Cargo.toml              # Workspace and shared dependencies
├── Cargo.lock              # Locked Rust dependency graph
├── CODEOWNERS              # Human-review ownership boundaries
├── UPSTREAM.md             # Adaptation provenance ledger
└── justfile                # Standard local gate commands
```

## Directory Purposes

**`.github/workflows/`:**
- Purpose: Enforce cross-platform quality and package release workflows.
- Contains: GitHub Actions YAML.
- Key files: `.github/workflows/gate.yml`, `.github/workflows/release.yml`.

**`contracts/`:**
- Purpose: Store human- and machine-readable frozen public vocabulary.
- Contains: Matching `.md`/`.json` files for journal semantics, event types, endpoints, and capabilities.
- Key files: `contracts/journal-semantics.json`, `contracts/event-types.json`, `contracts/flux-endpoint-contract.json`.

**`crates/`:**
- Purpose: Hold every Rust workspace package.
- Contains: One kebab-case directory per package with `Cargo.toml`, `src`, and optional `tests`, `fixtures`, `examples`, `contracts`, or `data`.
- Key files: `crates/nano-cli/src/main.rs`, `crates/nano-agent/src/turn.rs`, `crates/nano-session/src/op.rs`.

**`crates/nano-cli/`:**
- Purpose: Own binaries, parsing, adapter state, and production composition.
- Contains: ACP/protocol/exec hosts, provider keys, routing, session/plugin/rules/verify commands.
- Key files: `crates/nano-cli/src/main.rs`, `crates/nano-cli/src/acp_mode.rs`, `crates/nano-cli/src/exec_run.rs`, `crates/nano-cli/src/host_mode.rs`.

**`crates/nano-agent/`:**
- Purpose: Own provider- and transport-neutral agent orchestration.
- Contains: Bootstrap, turns, goals, tasks, cron, compaction, MCP, skills context, CUA, wiring.
- Key files: `crates/nano-agent/src/bootstrap.rs`, `crates/nano-agent/src/turn.rs`, `crates/nano-agent/src/wiring.rs`, `crates/nano-agent/src/tasks.rs`.

**`crates/nano-core/`:**
- Purpose: Own shared types without provider, network, or OS implementation knowledge.
- Contains: Budgets, permissions, execution rules, policy engine, search vocabulary, sensitive paths.
- Key files: `crates/nano-core/src/lib.rs`, `crates/nano-core/src/permissions.rs`, `crates/nano-core/src/policy_engine.rs`.

**`crates/nano-session/`:**
- Purpose: Own append-only session durability and projections.
- Contains: Operations, JSONL reader/writer, replay, coordinator, locks, compaction, attachments, forks.
- Key files: `crates/nano-session/src/op.rs`, `crates/nano-session/src/replay.rs`, `crates/nano-session/src/coordinator.rs`, `crates/nano-session/src/writer.rs`.

**`crates/nano-memory/`:**
- Purpose: Own project/agent-partitioned long-term memory.
- Contains: SQLite schema/store, deterministic resolver, hashed embedder, mediation, fixture/tests.
- Key files: `crates/nano-memory/src/store.rs`, `crates/nano-memory/src/schema.rs`, `crates/nano-memory/src/types.rs`, `crates/nano-memory/fixtures/memory-retrieval-recall-v1.json`.

**`crates/nano-model/`:**
- Purpose: Isolate provider wire formats and normalize model behavior.
- Contains: Flux/OpenAI/Anthropic clients, SSE, retry, catalogs, pricing, auth, fixtures.
- Key files: `crates/nano-model/src/types.rs`, `crates/nano-model/src/provider_catalog.rs`, `crates/nano-model/data/providerCatalog.vendored.json`.

**`crates/nano-tools/`:**
- Purpose: Own policy-aware agent effects.
- Contains: Filesystem, shell, search, repomap, web, image, and PTY tools.
- Key files: `crates/nano-tools/src/lib.rs`, `crates/nano-tools/src/fs.rs`, `crates/nano-tools/src/shell.rs`.

**`crates/nano-sandbox/`:**
- Purpose: Own native process containment and platform helpers.
- Contains: Shared translation plus Windows, Linux, and macOS modules/binaries.
- Key files: `crates/nano-sandbox/src/lib.rs`, `crates/nano-sandbox/src/windows.rs`.

**`crates/nano-egress/`:**
- Purpose: Own deny-by-default network authorization and credential-safe HTTP.
- Contains: Client, grants, policy, redaction.
- Key files: `crates/nano-egress/src/client.rs`, `crates/nano-egress/src/grant.rs`, `crates/nano-egress/src/policy.rs`.

**Extension crates:**
- Purpose: Keep optional capabilities behind explicit boundaries.
- Contains: MCP (`crates/nano-mcp`), skills (`crates/nano-skills`), plugins (`crates/nano-plugins`), hooks (`crates/nano-hooks`), checkpoints (`crates/nano-checkpoints`), CUA (`crates/nano-cua`).
- Key files: `crates/nano-mcp/src/lib.rs`, `crates/nano-skills/src/lib.rs`, `crates/nano-plugins/src/lib.rs`, `crates/nano-hooks/src/lib.rs`.

**Client and evidence crates:**
- Purpose: Render UI and validate capability claims.
- Contains: TUI (`crates/nano-tui`), gate engine (`crates/nano-verify`), repomap (`crates/nano-repomap`), protocol (`crates/nano-protocol`).
- Key files: `crates/nano-tui/src/app.rs`, `crates/nano-verify/src/gate.rs`, `crates/nano-protocol/src/host.rs`.

**`gates/`:**
- Purpose: Define executable acceptance cards and validate evidence.
- Contains: `registry.json`, per-gate `card.md`/`gate.cjs`, fixtures, JS libraries, tests.
- Key files: `gates/registry.json`, `gates/lib/contract.cjs`, `gates/tests/validate-evidence.cjs`.

**`scripts/`:**
- Purpose: Provide external proof and release-evidence harnesses.
- Contains: Soak, canary, CUA, sandbox, human, provisioning, Flux, collection scripts.
- Key files: `scripts/soak/soak.mjs`, `scripts/collect-evidence.ps1`, `scripts/c12-proof/Test-C12Proof.ps1`.

**`vendor/`:**
- Purpose: Preserve pinned donor source trees byte-for-byte.
- Contains: Rust crates and upstream assets/tests.
- Key files: `vendor/NOTICE`, `vendor/LICENSE`; provenance is in `UPSTREAM.md`.

## Key File Locations

**Entry Points:**
- `crates/nano-cli/src/main.rs`: Primary command dispatcher.
- `crates/nano-cli/src/bin/acp_profile/main.rs`: ACP profile/proof helper.
- `crates/nano-tui/src/main.rs`: Interactive terminal UI.
- `crates/nano-tools/src/bin/wayland-nano-pty-guard.rs`: PTY child supervisor.
- `crates/nano-mcp/src/bin/wayland-nano-mcp-fake-server/main.rs`: MCP test server.

**Configuration:**
- `Cargo.toml`: Workspace membership, edition, dependencies, release profile.
- `Cargo.lock`: Exact dependency resolution.
- `rust-toolchain.toml`: Pinned Rust toolchain.
- `clippy.toml`: Lint configuration.
- `deny.toml`: Dependency policy.
- `justfile`: Canonical gate aliases.
- `AGENTS.md`: Scope, security, provenance, completion rules.
- `ARCHITECTURE.md`: Architecture constitution.

**Core Logic:**
- `crates/nano-agent/src/turn.rs`: Model/tool loop.
- `crates/nano-agent/src/bootstrap.rs`: Session creation/resume and ownership.
- `crates/nano-agent/src/wiring.rs`: Tool definitions and dispatch.
- `crates/nano-cli/src/exec_run.rs`: One-shot host composition.
- `crates/nano-cli/src/acp_mode.rs`: Interactive ACP composition.
- `crates/nano-session/src/op.rs`: Durable vocabulary.
- `crates/nano-session/src/replay.rs`: Session projection.
- `crates/nano-memory/src/store.rs`: Memory writes, retrieval, retention, rebuild.
- `crates/nano-memory/src/schema.rs`: Memory schema.

**Testing:**
- `crates/*/src/tests.rs` and `#[cfg(test)]` modules: Unit tests near private implementation.
- `crates/*/tests/*.rs`: Public-contract and integration tests.
- `crates/nano-tui/tests/snapshots/*.snap`: TUI snapshots.
- `crates/nano-memory/tests/retrieval_recall.rs`: Recall and isolation acceptance.
- `crates/nano-memory/tests/durability.rs`: Kill-mid-write rebuild acceptance.
- `crates/nano-memory/tests/write_mediation.rs`: Write-mediation acceptance.
- `gates/tests/*.cjs`: Gate schema/evidence/adversarial validation.

## Naming Conventions

**Files:**
- Use Rust `snake_case.rs`: `provider_router.rs`, `attachment_store.rs`.
- Use `main.rs` only for binaries and `lib.rs` for crate exports.
- Use descriptive integration tests: `adversarial_journal.rs`, `retrieval_recall.rs`.
- Pair contracts by stem as `.md` plus `.json`: `contracts/event-types.md` and `contracts/event-types.json`.
- Use kebab-case for gate/script directories: `gates/install-payload`, `scripts/human-harness`.

**Directories:**
- Name workspace packages `nano-<capability>`: `crates/nano-memory`.
- Put platform implementations under `src/backends/` or platform-named modules: `crates/nano-cua/src/backends/windows.rs`.
- Put immutable inputs under `fixtures/`, `data/`, or purpose-specific fixture roots.

## Where to Add New Code

**New CLI or host command:**
- Primary code: focused module under `crates/nano-cli/src/` and thin dispatch in `crates/nano-cli/src/main.rs`.
- Tests: `crates/nano-cli/tests/` for process/public behavior or `crates/nano-cli/src/*_tests.rs` for injected host behavior.

**New agent orchestration capability:**
- Primary code: focused module under `crates/nano-agent/src/`, exported by `crates/nano-agent/src/lib.rs`.
- Production wiring: `crates/nano-agent/src/wiring.rs` and host factories in `crates/nano-cli/src/` only when runtime-facing.
- Tests: co-located private tests or named `crates/nano-agent/src/*_tests.rs`.

**New session transition:**
- Vocabulary: extend `crates/nano-session/src/op.rs` additively without renaming tags.
- Projection: update `crates/nano-session/src/replay.rs` for live state; explicitly match replay-neutral receipts.
- Contract: update governed files in `contracts/` only when public vocabulary changes.
- Tests: serialization/golden tests in `crates/nano-session/src/tests.rs` and integration tests in `crates/nano-session/tests/`.

**Core memory integration or feature:**
- Types/policy: `crates/nano-memory/src/types.rs`.
- Schema: `crates/nano-memory/src/schema.rs`; keep `(project, agent_id)` explicit.
- Journal/storage/rebuild: `crates/nano-memory/src/store.rs` plus additive `crates/nano-session/src/op.rs` operations.
- Runtime composition: explicit seams in `crates/nano-agent` and `crates/nano-cli`; do not assume `crates/nano-agent/src/memory.rs` is the same store.
- Tests/fixtures: `crates/nano-memory/tests/`, `crates/nano-memory/fixtures/`.

**New model provider behavior:**
- Universal types: `crates/nano-model/src/types.rs` only when provider-neutral.
- Provider module: focused file under `crates/nano-model/src/`.
- Catalog authority: `crates/nano-model/data/providerCatalog.vendored.json` and validation tests.
- Credentials: follow `crates/nano-cli/src/provider_key.rs`; add no new channel.

**New policy-enforced effect:**
- Permission vocabulary: `crates/nano-core/src/permissions.rs`.
- Tool: `crates/nano-tools/src/`.
- Definition/dispatch: `crates/nano-agent/src/wiring.rs`.
- OS support: `crates/nano-sandbox/src/` or `crates/nano-platform/src/lib.rs`.
- Network: `crates/nano-egress/src/client.rs`.

**New extension capability:**
- MCP: `crates/nano-mcp/src/`.
- Skills: `crates/nano-skills/src/`.
- Plugins: `crates/nano-plugins/src/`.
- Hooks: `crates/nano-hooks/src/`.
- Agent composition remains in `crates/nano-agent`; host configuration remains in `crates/nano-cli`.

**New acceptance gate:**
- Registry: `gates/registry.json`.
- Card/executable: `gates/<gate-name>/card.md`, `gates/<gate-name>/gate.cjs`.
- Fixtures/tests: `gates/<gate-name>/fixtures/`, `gates/tests/`.
- Generic Rust behavior: `crates/nano-verify/src/`.

**Utilities:**
- Keep helpers in the owning crate; there is no general workspace utility crate.
- Put only provider/OS-neutral vocabulary in `crates/nano-core`.

## Special Directories

**`.planning/`:**
- Purpose: Ferrox project state and codebase intelligence.
- Generated: Yes.
- Committed: Yes when incorporated by the orchestrator.

**`vendor/`:**
- Purpose: Immutable donor snapshots.
- Generated: No.
- Committed: Yes.

**`crates/nano-model/fixtures-flux/`:**
- Purpose: Recorded provider request/response evidence and parser fixtures.
- Generated: Captured by controlled probes.
- Committed: Yes.

**`scripts/*/evidence/`:**
- Purpose: Proof outputs and manifests required by owning harnesses.
- Generated: Yes.
- Committed: Mixed; follow local `.gitignore` and harness contract.

**`target/`:**
- Purpose: Cargo output; external `CARGO_TARGET_DIR` is supported.
- Generated: Yes.
- Committed: No.

---

*Structure analysis: 2026-08-27*
