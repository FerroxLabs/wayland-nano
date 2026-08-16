# Coding Conventions

**Analysis Date:** 2026-08-16

## Naming Patterns

**Files:**
- Use `snake_case.rs` for Rust modules and integration-test targets: `crates/nano-core/src/policy_engine.rs`, `crates/nano-model/tests/adversarial_sse.rs`.
- Use a crate-local `tests.rs` or a purpose-named `*_tests.rs` module for white-box tests that need private APIs: `crates/nano-agent/src/compact/tests.rs`, `crates/nano-session/src/fork_tests.rs`.
- Name binaries and external identities with the Wayland Nano namespace; binary sources such as `crates/nano-tools/src/bin/wayland-nano-pty-guard.rs` must not reuse Track A names.
- Keep generated, vendored, and donor-derived names unchanged where provenance requires it; record adapted files in `UPSTREAM.md`.

**Functions:**
- Use `snake_case` and choose behavior-oriented names: `canonicalize_preserving_symlinks` in `crates/nano-core/src/policy_engine.rs`, `effective_redirect_method` in `crates/nano-egress/src/client.rs`.
- Name tests as complete behavioral claims, not generic labels: `oversized_single_frame_is_rejected_with_typed_error` in `crates/nano-model/tests/adversarial_sse.rs`.
- Prefix narrowly scoped constructors/helpers conventionally with `new`, `from_*`, `build_*`, or the domain verb; examples are `FakeConnection::from_script` in `crates/nano-tui/tests/support/mod.rs` and `build_glob_matcher` in `crates/nano-core/src/policy_engine.rs`.

**Variables:**
- Use `snake_case`; give policy and security state explicit domain names (`follow_redirects`, `current_method`, `path_query_sha256`) as in `crates/nano-egress/src/client.rs`.
- Use uppercase `SCREAMING_SNAKE_CASE` for constants such as `MAX_REDIRECT_HOPS` in `crates/nano-egress/src/client.rs` and `MAX_SSE_FRAME_BYTES` in `crates/nano-model/src/sse.rs`.
- Preserve units in names where ambiguity matters (`duration_ms`, `input_tokens`, `width`, `height`), as exercised in `crates/nano-cua/tests/live.rs` and `crates/nano-agent/src/compact/tests.rs`.

**Types:**
- Use `UpperCamelCase` for structs, enums, traits, and variants: `EgressClient`, `EgressError`, `RedirectGate` in `crates/nano-egress/src/client.rs`.
- Use typed error enums with domain-specific variants instead of string-only control flow; derive `thiserror::Error` where appropriate, following `EgressError` in `crates/nano-egress/src/client.rs`.
- Keep public vocabulary in its owning crate. Shared policy and permission types belong in `crates/nano-core/src/`; outbound HTTP types belong in `crates/nano-egress/src/`.

## Code Style

**Formatting:**
- Use the pinned Rust formatter from `rust-toolchain.toml`; do not hand-format around it.
- Verify formatting with `just gate-fmt`, which runs `cargo fmt --all -- --check` from `justfile`.
- Follow standard rustfmt output: four-space indentation, trailing commas in multiline constructs, and formatter-controlled wrapping. Representative output is visible in `crates/nano-agent/src/compact/tests.rs`.

**Linting:**
- Run `just gate-lint`; `justfile` defines this as `cargo clippy --workspace --all-targets -- -D warnings`.
- Treat every warning as an error. Scope an `#[allow(...)]` to the smallest justified module and explain the invariant, as `#![allow(clippy::disallowed_methods)]` does in `crates/nano-egress/src/client.rs`.
- Never construct `reqwest::Client` outside `nano-egress`; `clippy.toml` bans `reqwest::Client::new`, `reqwest::Client::builder`, and `reqwest::get` workspace-wide.
- Use Rust edition 2024 and the pinned 1.95.0 toolchain declared by `Cargo.toml` and `rust-toolchain.toml`.

## Import Organization

**Order:**
1. Put module attributes and module-level documentation first (`//!`, then `#![...]`), following `crates/nano-egress/src/client.rs`.
2. Import local crate modules with `crate::...` or `super::*` when a tightly coupled test module needs the parent surface, as in `crates/nano-agent/src/compact/tests.rs`.
3. Import workspace/external crates next (`nano_model`, `nano_session`, `ratatui`).
4. Import `std` items last; split imports one-per-line when that matches the module, as in `crates/nano-tui/tests/support/mod.rs`.
- Let rustfmt normalize ordering inside groups; avoid wildcard imports in implementation code. `use super::*` is established for private white-box test modules only.

**Path Aliases:**
- Rust crate names are the only aliases: hyphenated package `nano-model` is imported as `nano_model`, demonstrated in `crates/nano-model/tests/adversarial_sse.rs`.
- No source-level filesystem path alias system is detected. Use `crate::`, `super::`, and dependency crate roots.

## Error Handling

**Patterns:**
- Return typed `Result` errors across fallible boundaries; reserve panics and `expect` for invariant construction or tests. `EgressError` in `crates/nano-egress/src/client.rs` is the model for a public boundary.
- Fail closed for sandbox, egress, policy, journal, parsing caps, and credential handling. Tests in `crates/nano-model/tests/adversarial_sse.rs` assert typed rejection and poisoned-state behavior after limit violations.
- Sanitize provider-controlled and credential-bearing text before it reaches `Display`, `Debug`, protocol frames, or logs; canaries in `crates/nano-model/tests/canary_redaction.rs` enforce this boundary.
- Recover poisoned mutexes deliberately only where continued operation is intended, using `unwrap_or_else(PoisonError::into_inner)` as in `crates/nano-tui/tests/support/mod.rs`.
- Never silently downgrade a missing security capability. Surface a typed refusal; the standing constraint is documented in `AGENTS.md`.

## Logging

**Framework:** `tracing` for structured library diagnostics; namespaced `eprintln!` for CLI/process-boundary diagnostics.

**Patterns:**
- Log structured fields without secrets, request bodies, headers, or raw sensitive paths. The outbound observability contract is documented in `crates/nano-egress/src/client.rs`.
- Prefix user-visible stderr with `wayland-nano` or the binary identity, as in `crates/nano-tools/src/bin/wayland-nano-pty-guard.rs`.
- Use `tracing::warn!` for recoverable library conditions, following `crates/nano-core/src/policy_engine.rs` and `crates/nano-hooks/src/lib.rs`.
- In tests, use `eprintln!` for explicit skip reasons and diagnostic context only; examples are `crates/nano-cua/tests/live.rs` and `crates/nano-agent/tests/c2_fixture.rs`.

## Comments

**When to Comment:**
- Put crate/module contracts in `//!` docs, including ownership boundaries and security invariants, as in `crates/nano-core/src/lib.rs` and `crates/nano-egress/src/lib.rs`.
- Explain why a constraint exists, especially fail-closed decisions, caps, platform gates, and donor deviations. Do not narrate obvious syntax.
- Preserve checkpoint/spec references where they make an externally verified requirement traceable (`C8 §8/§9`, `P3 §6.3`), following `crates/nano-model/tests/canary_redaction.rs` and `crates/nano-egress/src/client.rs`.

**JSDoc/TSDoc:**
- Not applicable. Use Rust doc comments: `///` for public items and `//!` for module/crate contracts.

## Function Design

**Size:** Keep helpers single-purpose and extract policy decisions into named functions. `effective_redirect_method` and `RedirectGate::gate_hop` in `crates/nano-egress/src/client.rs` separate redirect semantics from transport orchestration.

**Parameters:**
- Borrow inputs (`&str`, `&Path`, `&ModelRequest`) unless ownership is required.
- Use domain structs/enums instead of boolean clusters at public boundaries. When a boolean is retained, document its exact security meaning, as `follow_redirects` is documented in `crates/nano-egress/src/client.rs`.

**Return Values:**
- Return `Result<T, DomainError>` for fallible work and `Option<T>` for genuine absence.
- Prefer typed enum outcomes over sentinel strings; tests should match variants with `matches!`, as in `crates/nano-cua/tests/live.rs`.

## Module Design

**Exports:**
- Keep `lib.rs` thin and explicit with `pub mod` declarations, as in `crates/nano-core/src/lib.rs` and `crates/nano-egress/src/lib.rs`.
- Keep private helpers private; make a symbol `pub` only when another crate or integration test needs the supported contract.
- Respect architectural ownership: all outbound HTTP flows through `crates/nano-egress/`; shared vocabulary stays in `crates/nano-core/`; provider code stays in `crates/nano-model/`.

**Barrel Files:**
- Rust `lib.rs` files serve as explicit module barrels. Do not add glob re-exports without an established crate-level API reason.
- Test support may use a local `mod support;`, as in `crates/nano-tui/tests/l1_snapshots.rs`, with helpers in `crates/nano-tui/tests/support/mod.rs`.

---

*Convention analysis: 2026-08-16*
