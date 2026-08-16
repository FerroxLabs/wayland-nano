# Codebase Structure

**Analysis Date:** 2026-08-16

## Directory Layout

```
waylandnano/
├── wayland-nano/              # Active Track B Rust workspace
│   ├── crates/                # Product crates, one bounded responsibility each
│   ├── docs/                  # Compatibility, compliance, status, metrics, release docs
│   ├── scripts/               # Proof, provisioning, canary, soak, and release automation
│   ├── packaging/npm/         # Zero-dependency binary npm distribution
│   ├── vendor/                # Pinned donor-derived source trees
│   ├── Cargo.toml             # Workspace members and shared dependency policy
│   ├── ARCHITECTURE.md        # Architecture constitution
│   ├── UPSTREAM.md            # File-level provenance ledger
│   └── AGENTS.md              # Binding repository rules
├── shared/                    # Active cross-track contracts, fixtures, scorecard, reviews
│   ├── contracts/             # Machine/human-readable interface authorities
│   ├── fixtures/              # Recorded Desktop and Flux evidence inputs
│   ├── reviews/               # Claims, verdicts, plans, and evidence bundles
│   └── SCORECARD.md           # Checkpoint and promotion authority
├── nano/                      # Read-only Track A donor/comparison context
├── resources/upstreams/       # Read-only immutable upstream snapshots
└── .planning/codebase/        # Generated maps consumed by Ferrox planning
```

## Directory Purposes

**`wayland-nano/crates/`:**
- Purpose: Active implementation split along policy, orchestration, adapter, and host boundaries.
- Contains: Rust library and binary crates named `nano-*`.
- Key files: `wayland-nano/crates/nano-cli/src/main.rs`, `wayland-nano/crates/nano-agent/src/turn.rs`, `wayland-nano/crates/nano-session/src/lib.rs`

**`wayland-nano/crates/nano-cli/`:**
- Purpose: Product composition root and command-facing adapters.
- Contains: Main binary, ACP/NDJSON/exec modes, provider and MCP config, session commands, plugin commands.
- Key files: `wayland-nano/crates/nano-cli/src/main.rs`, `wayland-nano/crates/nano-cli/src/acp_mode.rs`, `wayland-nano/crates/nano-cli/src/host_mode.rs`

**`wayland-nano/crates/nano-agent/`:**
- Purpose: Host-neutral execution orchestration.
- Contains: Turn engine, driver/executor traits, loop protection, goals, tasks, memory, checkpoints, CUA, hooks and skills integration.
- Key files: `wayland-nano/crates/nano-agent/src/turn.rs`, `wayland-nano/crates/nano-agent/src/wiring.rs`, `wayland-nano/crates/nano-agent/src/tasks.rs`

**`wayland-nano/crates/nano-core/`:**
- Purpose: Dependency-light policy and shared invariant types.
- Contains: Permissions, paths, sensitive-file rules, budgets, search and execution rules.
- Key files: `wayland-nano/crates/nano-core/src/permissions.rs`, `wayland-nano/crates/nano-core/src/policy_engine.rs`

**`wayland-nano/crates/nano-model/`:**
- Purpose: Provider-neutral model API and provider wire adapters.
- Contains: Request/event types, Flux surfaces, provider catalog, SSE/retry/rate-limit/metering support.
- Key files: `wayland-nano/crates/nano-model/src/types.rs`, `wayland-nano/crates/nano-model/src/provider_catalog.rs`, `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json`

**`wayland-nano/crates/nano-tools/`:**
- Purpose: Policy-enforced agent tool implementations.
- Contains: Filesystem, search, shell, PTY, web, image, and repository-map tools.
- Key files: `wayland-nano/crates/nano-tools/src/fs.rs`, `wayland-nano/crates/nano-tools/src/shell.rs`, `wayland-nano/crates/nano-tools/src/lib.rs`

**`wayland-nano/crates/nano-sandbox/`:**
- Purpose: Native OS process containment and cleanup.
- Contains: Windows restricted-token/DACL/Job/WFP implementation, macOS Seatbelt, Linux Landlock/seccomp/bwrap, helper binaries.
- Key files: `wayland-nano/crates/nano-sandbox/src/lib.rs`, `wayland-nano/crates/nano-sandbox/src/wrapper.rs`, `wayland-nano/crates/nano-sandbox/src/linux_bwrap.rs`

**`wayland-nano/crates/nano-session/`:**
- Purpose: Append-only durable session state.
- Contains: Operation schema, coordinator, writer/reader, replay, recovery, fork, compaction and attachments.
- Key files: `wayland-nano/crates/nano-session/src/op.rs`, `wayland-nano/crates/nano-session/src/coordinator.rs`, `wayland-nano/crates/nano-session/src/replay.rs`

**`wayland-nano/crates/nano-protocol/`:**
- Purpose: Desktop-facing wire contracts and host framing.
- Contains: NDJSON messages/codec/host, ACP translation, capabilities, error codes, permission modes and replay corpus.
- Key files: `wayland-nano/crates/nano-protocol/src/host.rs`, `wayland-nano/crates/nano-protocol/src/messages.rs`, `wayland-nano/crates/nano-protocol/src/profile.rs`

**`wayland-nano/crates/nano-mcp/`, `nano-skills/`, `nano-plugins/`, `nano-hooks/`, `nano-checkpoints/`, `nano-cua/`:**
- Purpose: Optional capabilities behind explicit boundaries.
- Contains: MCP transports/registry, skill parsing, plugin store/plans, lifecycle hooks, checkpoint store, computer-use policy/backend.
- Key files: `wayland-nano/crates/nano-mcp/src/client.rs`, `wayland-nano/crates/nano-skills/src/loader.rs`, `wayland-nano/crates/nano-plugins/src/store.rs`

**`wayland-nano/docs/`:**
- Purpose: Product and verification reference material.
- Contains: `audits/`, `compliance/`, `metrics/`, `release/`, and `spikes/`.
- Key files: `wayland-nano/docs/STATUS.md`, `wayland-nano/docs/compliance/SCENARIO_CATALOG.md`, `wayland-nano/docs/release/EVIDENCE-BUNDLE.md`

**`shared/`:**
- Purpose: Active cross-track authority outside implementation crates.
- Contains: Contracts, recorded fixtures, checkpoint claims/verdicts, comparison reviews and scorecard.
- Key files: `shared/SCORECARD.md`, `shared/contracts/event-types.md`, `shared/contracts/journal-semantics.md`

## Key File Locations

**Entry Points:**
- `wayland-nano/crates/nano-cli/src/main.rs`: Primary command dispatcher.
- `wayland-nano/crates/nano-tui/src/main.rs`: Terminal UI binary.
- `wayland-nano/crates/nano-cli/src/bin/`: Helper/proof/setup binary entry points.
- `wayland-nano/crates/nano-sandbox/src/bin/`: Platform sandbox helper binaries.

**Configuration:**
- `wayland-nano/Cargo.toml`: Workspace membership, edition, shared dependencies and release profile.
- `wayland-nano/rust-toolchain.toml`: Pinned Rust toolchain and target components.
- `wayland-nano/justfile`: Local gate, proof, packaging, and verification recipes.
- `wayland-nano/deny.toml`: Dependency license/advisory policy.
- `wayland-nano/clippy.toml`: Workspace lint configuration.
- `wayland-nano/.github/workflows/`: CI workflows.

**Core Logic:**
- `wayland-nano/crates/nano-agent/src/turn.rs`: Agent turn state machine.
- `wayland-nano/crates/nano-agent/src/wiring.rs`: Production model/tool adapters.
- `wayland-nano/crates/nano-core/src/policy_engine.rs`: Permission decisions.
- `wayland-nano/crates/nano-session/src/coordinator.rs`: Ordered journal mutation.
- `wayland-nano/crates/nano-protocol/src/host.rs`: NDJSON host loop.

**Testing:**
- `wayland-nano/crates/*/tests/`: Crate integration tests and fixtures.
- `wayland-nano/crates/*/src/*_tests.rs`: Larger co-located test modules.
- `wayland-nano/scripts/c12-proof/`: C1.2 security proof harness.
- `wayland-nano/scripts/c11-proof/`: Headless/session proof harness.
- `shared/fixtures/`: Recorded external fixtures used as evidence.
- `shared/reviews/`: Checkpoint claims, verdicts, manifests, and proof artifacts.

## Naming Conventions

**Files:**
- Use `snake_case.rs` for Rust modules: `provider_router.rs`, `loop_protection.rs`.
- Use `*_tests.rs` for substantial test modules: `turn_tests.rs`, `p3_tests.rs`.
- Use `UPPERCASE.md` for authoritative project/control documents: `ARCHITECTURE.md`, `SCORECARD.md`.
- Use descriptive kebab-case for contracts and design notes: `journal-semantics.md`, `windows-hardlink-containment.md`.

**Directories:**
- Use `nano-<capability>` for workspace crates: `wayland-nano/crates/nano-egress/`.
- Use feature/checkpoint identifiers for proof suites: `wayland-nano/scripts/c12-proof/`, `shared/reviews/C3/`.
- Namespace product-created binaries/directories as `wayland-nano-*`; reserve `nano/` for the read-only Track A repository (`wayland-nano/AGENTS.md`).

## Where to Add New Code

**New Host Command or Product Mode:**
- Primary code: add a focused module under `wayland-nano/crates/nano-cli/src/` and keep dispatch thin in `wayland-nano/crates/nano-cli/src/main.rs`.
- Tests: use `wayland-nano/crates/nano-cli/src/*_tests.rs` for internal seams or `wayland-nano/crates/nano-cli/tests/` for binary/integration behavior.

**New Agent Behavior:**
- Primary code: add a focused module under `wayland-nano/crates/nano-agent/src/` and expose only the required surface from `wayland-nano/crates/nano-agent/src/lib.rs`.
- Tests: colocate focused unit tests or add a named `*_tests.rs` module under `wayland-nano/crates/nano-agent/src/`.

**New Tool:**
- Implementation: add `wayland-nano/crates/nano-tools/src/<tool>.rs`, enforce `nano-core` policy inside it, and register it through `wayland-nano/crates/nano-agent/src/wiring.rs` or a host wrapper.
- Tests: add tool-level tests under `wayland-nano/crates/nano-tools/tests/` or the module; add host proof only when the capability claim requires it.

**New Provider or Wire Surface:**
- Implementation: place provider-neutral types in `wayland-nano/crates/nano-model/src/types.rs`, provider adapter code in `wayland-nano/crates/nano-model/src/`, and host selection/config in `wayland-nano/crates/nano-cli/src/provider_router.rs`.
- Catalog changes: update only the authoritative vendored catalog path `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json` using its provenance rules.

**New Protocol Field or Event:**
- Shared authority: change the relevant file under `shared/contracts/` first when the cross-track contract changes.
- Implementation: update `wayland-nano/crates/nano-protocol/src/messages.rs`, codec/host adapters, and capability honesty in `wayland-nano/crates/nano-protocol/src/profile.rs`.

**New Platform Behavior:**
- Abstraction: add the portable boundary in `wayland-nano/crates/nano-platform/` only when required by the agent-facing contract.
- Containment: add target-specific execution details under `wayland-nano/crates/nano-sandbox/src/`; do not add OS branches to `nano-agent`.

**Utilities:**
- Shared policy/value helpers: `wayland-nano/crates/nano-core/src/`.
- Crate-private helpers: keep them in the owning crate instead of creating a cross-workspace utility crate.

**Verification Assets:**
- Deterministic proof automation: `wayland-nano/scripts/<checkpoint-or-feature>/`.
- Recorded external fixtures: `shared/fixtures/<service-or-protocol>/`.
- Claims and verdicts: `shared/reviews/<checkpoint>/`, following `shared/SCORECARD.md` ownership rules.

## Special Directories

**`wayland-nano/vendor/`:**
- Purpose: Pinned donor-derived crates and assets tracked by the provenance ledger.
- Generated: No.
- Committed: Yes; preserve byte identity where `wayland-nano/UPSTREAM.md` marks content vendored.

**`wayland-nano/target/`:**
- Purpose: Cargo build output.
- Generated: Yes.
- Committed: No.

**`wayland-nano/.tmp-wt-*`:**
- Purpose: Temporary worktrees when present.
- Generated: Yes.
- Committed: No; never inspect these as implementation truth.

**`nano/`:**
- Purpose: Track A comparison/donor repository.
- Generated: No.
- Committed: Independently managed; read-only from Track B work.

**`resources/upstreams/`:**
- Purpose: Immutable upstream donor snapshots and source index.
- Generated: No.
- Committed: Yes; read-only.

**`shared/reviews/`:**
- Purpose: Review plans, evidence, claims, verdicts, and owner promotion records.
- Generated: Mixed; evidence may be tool-generated, decisions are authored.
- Committed: Yes.

---

*Structure analysis: 2026-08-16*
