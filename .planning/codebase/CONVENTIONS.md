# Coding Conventions

**Analysis Date:** 2026-08-27

## Naming Patterns

**Files:**
- Use lowercase `snake_case.rs` for Rust modules and focused test files: `crates/nano-memory/src/write_mediation.rs`, `crates/nano-memory/tests/retrieval_recall.rs`.
- Keep crate directories and package names namespaced as `nano-*`: `crates/nano-session`, `crates/nano-memory`, `crates/nano-verify`.
- Use milestone/checkpoint names only when the file is an explicit acceptance surface: `crates/nano-cli/tests/c5_memory.rs`, `crates/nano-agent/tests/c2_kill_mid_edit.rs`.
- Store committed data fixtures below the owning crate's `fixtures/` or `tests/fixtures/`: `crates/nano-memory/fixtures/memory-retrieval-recall-v1.json`, `crates/nano-cli/tests/fixtures/verify/`.
- Name generated contract pairs identically with `.json` and `.md` extensions: `contracts/event-types.json`, `contracts/event-types.md`.
- Gate-card IDs, directories, scripts, fixtures, and registry keys must agree exactly: `gates/config-schema/card.md`, `gates/config-schema/launcher.cjs`, `gates/registry.json`.

**Functions:**
- Use `snake_case`; use verb-led names for operations and assertions: `commit_proposal`, `rebuild_from_journals`, `assert_no_resurrection`.
- Constructors use `new`, `open`, or explicit variants such as `open_at`; fallible operations return `Result`: `crates/nano-memory/src/store.rs`.
- Parsers use `parse`; stable string conversions use `as_str`: `SourceTrust::parse` and `SourceTrust::as_str` in `crates/nano-memory/src/types.rs`.
- Test names state behavior or invariant, not implementation: `explicit_agent_scope_never_widens_project`, `rebuild_ignores_unreceipted_model_write`.
- Test helpers stay private and narrowly named (`load`, `ingest`, `query_ids`) in `crates/nano-memory/tests/durability.rs`.

**Variables:**
- Use concise domain names where the type makes meaning clear (`store`, `fixture`, `receipt`, `journal`) and descriptive names at security boundaries (`expected_frontmost_app`, `first_affected_line`).
- Use uppercase `NANO_*` for Wayland Nano environment variables: `NANO_MEMORY_KILL_DB` in `crates/nano-memory/tests/durability.rs`.
- Never introduce unnamespaced persistent or environment identities; `AGENTS.md` requires `wayland-nano-*`, `NanoSandbox*`, `NANO_*`, and `nano.*`.

**Types:**
- Use `UpperCamelCase` for structs, enums, and traits: `MemoryStore`, `MemoryError`, `AgentScope`, `Clock`.
- Use semantic result aliases at crate boundaries when a crate has a typed error: `MemoryResult<T>` in `crates/nano-memory/src/types.rs`.
- Enum variants use `UpperCamelCase` and serialize to explicit contract spellings when necessary: `SourceTrust::{User, ToolOutput, ModelInference}`.
- Capability-specific data types carry the domain prefix when ambiguity is likely: `MemoryProposal`, `MemoryReceipt`, `RetrieveQuery`.

## Code Style

**Formatting:**
- Use the pinned Rust formatter from `rust-toolchain.toml`; run `cargo fmt --all -- --check` or `just gate-fmt`.
- Rust edition is workspace-wide 2024 in `Cargo.toml`; do not apply per-crate edition overrides.
- Let rustfmt decide wrapping and imports. Do not hand-align fields or retain dense one-line code merely to minimize line count.
- JavaScript under `gates/` is CommonJS with `'use strict';`, semicolons, two-space indentation, and Node built-ins via `require('node:...')`: `gates/tests/gates-card-schema.test.cjs`.
- PowerShell automation sets fail-fast behavior and validates exact owned paths before mutation: `.github/workflows/gate.yml`, `scripts/c12-proof/Test-C12Proof.ps1`.

**Linting:**
- Run `cargo clippy --workspace --all-targets -- -D warnings` via `just gate-lint`; warnings are errors.
- `clippy.toml` bans direct `reqwest::Client::new`, `reqwest::Client::builder`, and `reqwest::get`; outbound HTTP belongs in `crates/nano-egress`.
- Do not add blanket `allow` attributes. The documented narrow exception is local to the authorized egress client module.
- Dependency policy is a separate hard gate: `cargo deny check` configured by `deny.toml`.
- Generated artifacts must remain byte-fresh through `just gate-gen-check`, which checks `crates/nano-cli/src/bin/gen_error_table.rs` and `crates/nano-cli/src/bin/gen_contracts.rs`.

## Import Organization

**Order:**
1. Standard-library imports (`std::path::Path`, `std::sync::{Arc, Mutex}`).
2. External crates (`serde`, `rusqlite`, `tokio`).
3. Workspace crates (`nano_session`, `nano_memory`).
4. Current-crate modules (`crate::backend`, `crate::error`).

Examples vary slightly by file, so preserve rustfmt output and match the nearest module. `crates/nano-cua/src/mock.rs` groups standard imports before crate-local imports; `crates/nano-memory/src/mediation.rs` starts with its cohesive `crate::{...}` domain group, then `nano_session`.

**Path Aliases:**
- Rust uses Cargo crate names and `crate::`, `super::`, or `self::`; no source-path alias system is present.
- Prefer grouped imports for several items from one module: `use crate::{DecisionWrite, EpisodeWrite, ...};` in `crates/nano-memory/src/mediation.rs`.
- In integration tests, import the public surface from the crate root: `use nano_memory::*;` in `crates/nano-memory/tests/*.rs`.

## Error Handling

**Patterns:**
- Define domain errors with `thiserror::Error`, user-safe messages, and `#[from]` only for faithful lower-level conversion: `MemoryError` in `crates/nano-memory/src/types.rs`.
- Return typed errors from library code. Do not `unwrap` or panic on untrusted/runtime input; map parsing, I/O, policy, contention, and integrity failures to domain variants.
- Validate at the boundary and fail closed. `validate_partition` rejects empty projects and malformed agent IDs; `reject_network_path` rejects network filesystems in `crates/nano-memory/src/types.rs`.
- Preserve distinct security meanings rather than flattening errors: `MediationRequired`, `ScreeningRejected`, `NetworkFilesystem`, and `JournalIntegrity` in `crates/nano-memory/src/types.rs`.
- Use `?` for propagation and `map_err` only when crossing an abstraction boundary: secret screening in `crates/nano-memory/src/mediation.rs`, journal error conversion in `crates/nano-session/src/writer.rs`.
- Tests may use `unwrap`/`expect` for setup and assert precise error variants using `matches!`; include an assertion message where failure context is not obvious.
- Missing required subject matter is a test failure. Only explicitly live-gated credential tests may self-skip, per `AGENTS.md` and `justfile`.
- Never weaken sandbox, egress, policy, journal, or acceptance assertions to turn a valid failure green; preserve the failure and report it.

## Logging

**Framework:** Structured journal/events for durable behavior; `println!` only for deliberate test evidence and CLI output.

**Patterns:**
- Durable state transitions belong in the append-only journal owned by `crates/nano-session`, not ad-hoc logs.
- Acceptance tests print compact machine/human-readable evidence after assertions, for example recall and partition leakage in `crates/nano-memory/tests/retrieval_recall.rs`.
- Gate-card stdout is a closed protocol: zero or more `FAIL <ID> <category>` lines followed by exactly one `gate: N/M`; no PASS chatter (`gates/README.md`).
- Never log secrets. Screening and journal redaction use `crates/nano-session/src/redaction.rs`; fixtures and output must not contain credentials.
- Use `tracing`/existing event mechanisms only where the surrounding crate already does; do not introduce a second logging abstraction.

## Comments

**When to Comment:**
- Document invariants, trust boundaries, platform constraints, and the reason for a non-obvious implementation: `crates/nano-agent/src/clock.rs`, `crates/nano-cua/src/mock.rs`.
- Tie acceptance logic to a governing requirement/checkpoint when it improves traceability: `// C7:` and `// §5.1` patterns in `crates/nano-tui/tests/l1_snapshots.rs` and `crates/nano-cua/src/mock.rs`.
- Explain why a dependency or feature pin exists in `Cargo.toml`; do not add narrative history to ordinary code.
- Keep donor transformations in `UPSTREAM.md`; every adapted donor file needs destination, source, and exact transformation.

**Rustdoc:**
- Use `//!` for module purpose and invariants, especially security/test-support modules.
- Use `///` for public APIs and behavior that callers must preserve; `commit_proposal` documents its authority boundary in `crates/nano-memory/src/mediation.rs`.
- Avoid comments that merely restate syntax.

## Function Design

**Size:** Keep validation, screening, persistence, and retrieval steps separable and testable. Extract a helper when it names an invariant, not to create speculative abstraction. Examples: `screen` in `crates/nano-memory/src/mediation.rs` and `assert_no_resurrection` in `crates/nano-session/tests/adversarial_journal.rs`.

**Parameters:**
- Borrow read-only values (`&Path`, `&str`, `&RetrieveQuery`) and take ownership when mutation or durable storage needs it (`MemoryProposal`, `FactWrite`).
- Group policy knobs into domain structs such as `MemoryPolicy` rather than extending positional argument lists.
- Use `impl Into<String>` sparingly for ergonomic constructors/setters where no validation is bypassed: `TestClock` and `MockBackend` patterns in `crates/nano-agent/src/clock.rs` and `crates/nano-cua/src/mock.rs`.

**Return Values:**
- Return domain values/receipts on success and typed errors on failure: `MemoryStore::commit_proposal` returns `MemoryResult<MemoryReceipt>`.
- Return evidence-bearing structures instead of booleans when downstream code needs details: journal reports in `crates/nano-session/src/reader.rs`.
- Use booleans only for narrow predicates or idempotency outcomes.

## Module Design

**Exports:**
- Keep implementation modules private and re-export the intended public API from `src/lib.rs`: `crates/nano-memory/src/lib.rs` re-exports mediation, resolver, store, and types.
- Use `pub(crate)` for cross-module helpers that are not contract surface: validation and mediated commit helpers in `crates/nano-memory/src/types.rs` and `src/store.rs`.
- Place production-only and test-only implementations behind explicit `cfg` gates where needed: `crates/nano-cua/src/mock.rs` and platform backends.
- Add new workspace crates to the root `Cargo.toml` and inherit workspace package metadata/dependencies where available.

**Barrel Files:**
- Rust `lib.rs` is the deliberate crate facade; do not create broad convenience re-exports in arbitrary modules.
- Module declarations and re-exports must expose only the supported contract. Agent-facing memory integration must not bypass the mediation boundary in `crates/nano-memory/src/mediation.rs`.

## Security and Scope Discipline

- Match `AGENTS.md`: smallest complete change, no speculative hooks, no unrelated cleanup, and every line traceable to the assignment.
- Preserve project and agent partition arguments through all memory APIs; never invent a global/cross-project read path.
- Model-originated memory writes go through `MemoryStore::commit_proposal`; direct model-tier writes must return `MemoryError::MediationRequired`.
- All external HTTP goes through `crates/nano-egress`; all filesystem/process tools remain policy-enforced through their owning crates.
- Never read, print, or commit secret files. Live credentials are passed only through existing environment/file-reference resolution.

---

*Convention analysis: 2026-08-27*
