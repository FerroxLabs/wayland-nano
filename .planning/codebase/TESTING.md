# Testing Patterns

**Analysis Date:** 2026-08-16

## Test Framework

**Runner:**
- Rust built-in test harness via Cargo, pinned to Rust 1.95.0 by `rust-toolchain.toml`.
- Config: workspace membership and per-crate dev-dependencies are declared in `Cargo.toml` and `crates/*/Cargo.toml`; gate commands live in `justfile`.

**Assertion Library:**
- Built-in `assert!`, `assert_eq!`, `matches!`, `expect`, and `expect_err` are primary.
- `pretty_assertions` is enabled in domain crates such as `crates/nano-core/Cargo.toml`, `crates/nano-agent/Cargo.toml`, and `crates/nano-egress/Cargo.toml` for readable structural diffs.
- `insta` snapshots are used for TUI rendering via `crates/nano-tui/Cargo.toml` and `crates/nano-tui/tests/l1_snapshots.rs`.
- `tokio::test` drives async tests where runtime behavior is under test, including `crates/nano-cua/tests/live.rs`.

**Run Commands:**
```bash
cargo test --workspace                       # Run deterministic unit + integration tests
cargo test -p nano-model adversarial_sse     # Narrow by package/test filter while developing
cargo test -p nano-model -- --ignored        # Run ignored/live model proofs when prerequisites exist
just gate-all                                # Required local completion gate: fmt, lint, tests, generated drift
```

There is no configured watch-mode or coverage command in `justfile`. Do not claim numeric coverage targets; use requirement/scenario evidence instead.

## Test File Organization

**Location:**
- Put black-box and cross-module integration tests in `crates/<crate>/tests/*.rs`, for example `crates/nano-session/tests/adversarial_journal.rs` and `crates/nano-cli/tests/vertical_slice.rs`.
- Put white-box tests beside source under `src/tests.rs` or `src/*_tests.rs` when private implementation access is required, for example `crates/nano-agent/src/compact/tests.rs` and `crates/nano-session/src/fork_tests.rs`.
- Put reusable integration-test helpers under `crates/<crate>/tests/support/`, following `crates/nano-tui/tests/support/mod.rs`.
- Put recorded inputs under a test-local `fixtures/` directory and snapshots under `tests/snapshots/`, following `crates/nano-tui/tests/fixtures/` and `crates/nano-tui/tests/snapshots/`.
- Treat `shared/fixtures/flux/` as recorded external evidence. Replay fixtures in normal tests; do not require live network access in CI.

**Naming:**
- Name test files by behavior or proof surface: `adversarial_sse.rs`, `canary_redaction.rs`, `vertical_slice.rs`, `c10_proof.rs`.
- Name test functions as expected behavioral claims: `garbage_bytes_mid_stream_do_not_corrupt_neighbors` in `crates/nano-model/tests/adversarial_sse.rs`.

**Structure:**
```text
crates/<crate>/
├── src/
│   ├── <module>.rs
│   └── <module>/tests.rs       # private white-box tests
└── tests/
    ├── <behavior>.rs           # integration-test target
    ├── support/mod.rs          # shared harness, when needed
    ├── fixtures/               # deterministic recorded input
    └── snapshots/              # reviewable insta output
```

## Test Structure

**Suite Organization:**
```rust
//! State the requirement, invariant, and failure posture for the suite.

fn fixture_or_fake(...) -> ... { /* deterministic setup */ }

#[test]
fn oversized_single_frame_is_rejected_with_typed_error() {
    let mut parser = SseParser::new();
    let err = parser.feed(&hostile_input).expect_err("oversized frame must error");
    assert!(matches!(err, SseError::FrameTooLarge { .. }));
}
```
This pattern is implemented in `crates/nano-model/tests/adversarial_sse.rs`.

**Patterns:**
- Start suites with `//!` documentation that identifies the requirement and invariant, as in `crates/nano-model/tests/canary_redaction.rs`.
- Use Arrange/Act/Assert as readable phases without mandatory marker comments.
- Assert the typed error and the security consequence, not only `is_err()`. The SSE tests in `crates/nano-model/tests/adversarial_sse.rs` also verify poisoned parsers remain fail-closed.
- Give assertions diagnostic messages with relevant state (`{err:?}`, `{screen}`, expected invariant), as in `crates/nano-tui/tests/l1_snapshots.rs`.
- Clean up owned temporary resources or use `tempfile`; never touch another test's workspace. Several crates declare `tempfile` in their `Cargo.toml`.

## Mocking

**Framework:** Hand-written fakes, scripted drivers, local test servers/processes, and recorded fixtures; no general-purpose mocking framework is detected.

**Patterns:**
```rust
#[derive(Debug)]
struct FakeModel {
    responses: Mutex<Vec<Result<ModelResponse, ModelError>>>,
    requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait::async_trait]
impl ModelDriver for FakeModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        self.responses.lock().unwrap().remove(0)
    }
}
```
The fuller scripted model pattern is in `crates/nano-agent/src/compact/tests.rs`. The ACP/TUI scripted host pattern is in `crates/nano-tui/tests/support/mod.rs`.

**What to Mock:**
- Mock provider/model responses, clocks, tool executors, process peers, ACP/MCP peers, and terminal backends when the subject is local orchestration.
- Record calls and requests so tests can assert ordering and invariants, as `FakeModel.requests` does in `crates/nano-agent/src/compact/tests.rs`.
- Use recorded Flux exchanges from `shared/fixtures/flux/` for deterministic protocol compatibility tests.

**What NOT to Mock:**
- Do not mock the layer that owns the requirement under test. Egress policy tests must exercise `nano-egress`; journal integrity tests must exercise the real journal implementation.
- For end-to-end proofs, use the actual binary/process and external-state oracles. `crates/nano-cli/tests/vertical_slice.rs` checks protocol frames, child exit, and process inventory.
- Never weaken or replace sandbox, egress, policy, or journal enforcement to make a test pass; `AGENTS.md` makes those failures reportable evidence.

## Fixtures and Factories

**Test Data:**
```rust
const LIFECYCLE: &str = include_str!("fixtures/adversarial/lifecycle_only.ndjson");

let mut world = World::new(LIFECYCLE, 80, 24, None);
world.type_and_submit("/model");
insta::assert_snapshot!(render_to_test_backend(&world.app, 80, 24));
world.finish();
```
This exact fixture/world/snapshot style is used in `crates/nano-tui/tests/l1_snapshots.rs`.

**Location:**
- TUI protocol fixtures: `crates/nano-tui/tests/fixtures/`.
- TUI snapshots: `crates/nano-tui/tests/snapshots/`.
- Model replay fixtures: `crates/nano-model/fixtures-flux/` and authoritative shared captures in `shared/fixtures/flux/`.
- Redaction fixtures: `crates/nano-session/fixtures/redaction/`.
- Protocol corpus: `crates/nano-protocol/corpus/`.
- Keep fixtures deterministic, non-secret, and reviewable. Never embed live credentials; canary strings must be synthetic as in `crates/nano-model/tests/canary_redaction.rs`.

## Coverage

**Requirements:** No percentage target or instrumentation is enforced. Coverage is scenario- and invariant-driven through workspace tests, adversarial batteries, platform CI, checkpoint proof tests, and recorded fixtures.

**View Coverage:**
```bash
# Not configured in the repository. Add no coverage claim without introducing
# and validating a project-approved coverage tool and command.
```

The required gate is `just gate-all` from `justfile`; CI additionally runs `gate-deny` across the six-target matrix in `.github/workflows/gate.yml`.

## Test Types

**Unit Tests:**
- Test pure rules, parsers, typed errors, state transitions, builders, and private helpers close to source. Examples: `crates/nano-agent/src/compact/tests.rs`, `crates/nano-session/src/tests.rs`.
- Prefer exhaustive edge tables and adversarial cases for policy/security code.

**Integration Tests:**
- Exercise public crate APIs and boundaries under `crates/*/tests/`. Examples include filesystem/policy attacks in `crates/nano-tools/tests/adversarial_fs.rs`, journal integrity in `crates/nano-session/tests/adversarial_journal.rs`, and provider redaction in `crates/nano-model/tests/canary_redaction.rs`.
- Use watchdogs/timeouts for hostile inputs or subprocesses so a hang becomes a bounded failure. `run_guarded` in `crates/nano-model/tests/adversarial_sse.rs` uses a 30-second receive timeout.
- Use platform `#[cfg(...)]` gates for platform-specific implementation paths; keep deterministic cross-platform behavior in the normal workspace suite.

**E2E Tests:**
- Binary/protocol vertical slices live in `crates/nano-cli/tests/vertical_slice.rs`; TUI PTY proofs live in `crates/nano-tui/tests/pty.rs` and checkpoint-specific PTY files.
- Normal CI does not call live Flux services. `.github/workflows/gate.yml` runs recorded fixture replays and explicitly excludes owner-held credentials.
- Live proofs are explicit and self-skipping with a reason when prerequisites are absent. `crates/nano-cua/tests/live.rs` gates on `NANO_CUA_LIVE`; model/agent live paths gate on `FLUX_TEST_KEY` without printing it.

## Common Patterns

**Async Testing:**
```rust
#[tokio::test]
async fn behavior_is_enforced() {
    let result = subject.dispatch(...).await;
    assert!(matches!(result, Err(DomainError::ExpectedVariant)));
}
```
Use the crate's declared Tokio features and await the real async boundary. Platform-aware live examples are in `crates/nano-cua/tests/live.rs`.

**Error Testing:**
```rust
let err = parser
    .feed(&oversized)
    .expect_err("oversized frame must error");
assert_eq!(err, SseError::FrameTooLarge { limit: MAX_SSE_FRAME_BYTES });

let again = parser
    .feed("data: ok\n\n")
    .expect_err("poisoned parser must keep failing");
assert_eq!(again, err);
```
This fail-closed two-stage assertion comes from `crates/nano-model/tests/adversarial_sse.rs`.

**Snapshot Testing:**
- Render through a deterministic backend, assert with `insta::assert_snapshot!`, and commit the `.snap` file beside the suite under `crates/nano-tui/tests/snapshots/`.
- Call harness teardown assertions such as `World::finish()` so unconsumed frames or script violations fail loudly; see `crates/nano-tui/tests/l1_snapshots.rs` and `crates/nano-tui/tests/support/mod.rs`.

**Live and Ignored Tests:**
- Use `#[ignore = "reason"]` only for expensive, elevated, or harness-driven tests, as in `crates/nano-tools/src/fs.rs` and `crates/nano-sandbox/src/wfp.rs`.
- If a live prerequisite is absent, print a clear skip reason and return. If the subject matter should exist after the gate is enabled, fail rather than silently skip; `crates/nano-cua/tests/live.rs` demonstrates this distinction.

---

*Testing analysis: 2026-08-16*
