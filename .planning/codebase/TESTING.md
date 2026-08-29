# Testing Patterns

**Analysis Date:** 2026-08-27

## Test Framework

**Runner:**
- Rust built-in test harness under pinned Rust 1.95.0; configuration is `rust-toolchain.toml`, root `Cargo.toml`, and each `crates/*/Cargo.toml`.
- Tokio tests use `#[tokio::test]` for asynchronous agent, process, protocol, and integration behavior: `crates/nano-agent/tests/c2_fixture.rs`.
- Node 20 built-in `node:test` plus `node:assert/strict` tests gate-card infrastructure: `gates/tests/*.test.cjs`.
- `insta` snapshots review TUI rendering: `crates/nano-tui/tests/l1_snapshots.rs` and `crates/nano-tui/tests/snapshots/*.snap`.

**Assertion Library:**
- Rust standard `assert!`, `assert_eq!`, `matches!`, plus `insta::assert_snapshot!` for rendered UI.
- Node `node:assert/strict` for gate-card meta-tests.

**Run Commands:**
```bash
just gate-all                         # fmt + clippy -D warnings + workspace tests + generated-contract checks
cargo test --workspace               # all non-live Rust unit and integration tests
cargo test -p nano-memory             # P-MEM-1 crate tests
cargo test -p nano-memory --test retrieval_recall -- --nocapture
cargo test -p nano-memory --test durability -- --nocapture
cargo test -p nano-memory --test write_mediation -- --nocapture
just gate-deny                        # license, advisory, source, and dependency policy
just gate-live                        # owner-credential live suite; not normal CI
node --test gates/tests/*.test.cjs    # gate-card meta-tests where supported by the shell
```

No watch-mode or workspace coverage command is defined in `justfile`.

## Test File Organization

**Location:**
- Co-locate small unit tests in `src/*.rs` under `#[cfg(test)] mod tests` when they need private implementation access: `crates/nano-cua/src/mock.rs`.
- Put public API, adversarial, process, cross-module, and acceptance tests in `crates/<crate>/tests/*.rs`: `crates/nano-memory/tests/`, `crates/nano-session/tests/adversarial_journal.rs`.
- Large crate-internal batteries may use dedicated `src/*_tests.rs` modules wired from `lib.rs`: `crates/nano-agent/src/cron_tests.rs`, `crates/nano-session/src/p3_tests.rs`.
- Put deterministic fixture data in the owner crate: `crates/nano-memory/fixtures/`, `crates/nano-tui/tests/fixtures/`, `crates/nano-cli/tests/fixtures/verify/`.
- Put UI snapshots in `crates/nano-tui/tests/snapshots/`.
- Put repository-deliverable gate fixtures and mutants under `gates/fixtures/<gate-id>/`; their closure authority is `gates/registry.json`.

**Naming:**
- Name integration tests after behavior or acceptance package: `retrieval_recall.rs`, `write_mediation.rs`, `c2_kill_mid_edit.rs`.
- Name individual tests as complete behavioral claims: `model_proposes_host_commits_and_receipts`.
- Adversarial test names identify the attack and fail-closed result: `corrupted_final_line_is_dropped_never_partially_recovered`.
- Gate-card test IDs are stable lowercase strings (`t-card-schema-valid`) while checks use stable uppercase IDs (`CF-01`).

**Structure:**
```text
crates/<crate>/
├── src/
│   ├── *.rs
│   └── *_tests.rs             # large private/internal batteries
├── tests/
│   ├── *.rs                   # integration/adversarial/acceptance tests
│   ├── fixtures/              # test-owned inputs
│   └── snapshots/             # insta output when applicable
└── fixtures/                  # crate contract fixtures included at compile time

gates/
├── <gate-id>/card.md
├── <gate-id>/gate.*
├── fixtures/<gate-id>/{reference,mutants,...}
└── tests/*.test.cjs
```

## Test Structure

**Suite Organization:**
```rust
fn load() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/memory-retrieval-recall-v1.json")).unwrap()
}

#[test]
fn memory_retrieval_recall_v1_bar() {
    let fixture = load();
    assert_eq!(fixture.facts.len(), 50);
    let temp = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &temp.path().join("session.jsonl"),
        MemoryPolicy::default(),
    ).unwrap();
    // Exercise public behavior, assert partition invariant and metric threshold.
}
```
Pattern source: `crates/nano-memory/tests/retrieval_recall.rs`.

**Patterns:**
- Arrange deterministic state with small helpers; act through the public surface; assert output plus security/durability invariants.
- Assert fixture identity and cardinality before using its quality metric so fixture drift cannot silently change the bar.
- Use `tempfile::tempdir()` for isolated filesystem tests unless a governed platform proof requires an explicit owned volume.
- For fail-closed behavior, assert the exact typed error variant using `matches!`: `crates/nano-memory/tests/retrieval_recall.rs` and `write_mediation.rs`.
- Add assertion messages containing the violated invariant or loop variable in property-style batteries: `crates/nano-session/tests/adversarial_journal.rs`.
- Exercise serialization/replay through real journal APIs; do not replace durable state with in-memory mocks when recovery is the claim.
- Use `println!` only for concise acceptance evidence after correctness assertions.

## Mocking

**Framework:** Handwritten trait fakes and scripted backends; no general Rust mocking framework is established.

**Patterns:**
```rust
#[derive(Debug, Default)]
pub struct TestClock {
    now: std::sync::atomic::AtomicU64,
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }
}
```
Pattern source: `crates/nano-agent/src/clock.rs`.

The CUA test backend uses an `Arc<Mutex<MockState>>`, a FIFO `VecDeque<MockBehavior>`, recorded dispatches, and explicit `Fail`, `Panic`, and `Hang` behaviors in `crates/nano-cua/src/mock.rs`.

**What to Mock:**
- Time via `Clock`/`TestClock` to remove wall-clock flakes.
- Model/protocol peers with deterministic scripted transports or fake binaries when the contract is orchestration.
- OS interaction behind an existing trait, using a scripted backend that records calls and can force error/race states.
- Network service responses by replaying committed fixtures where endpoint behavior is already recorded.

**What NOT to Mock:**
- SQLite, FTS5, sqlite-vec, journal fsync/rebuild, or process death when storage durability/retrieval is the claim: `crates/nano-memory/tests/durability.rs`.
- Sandbox, filesystem aliasing, egress denial, or process-tree termination when containment is the claim: `crates/nano-core/tests/read_deny_alias.rs`, `crates/nano-tools/tests/pty_host_kill.rs`.
- Gate scripts or produced artifacts during gate-card dogfood; use the landed `wayland-nano verify` path described in `gates/README.md`.
- Security policy parsing at the gate layer; gates must invoke the shipped parser rather than duplicate it.

## Fixtures and Factories

**Test Data:**
```rust
#[derive(Deserialize)]
struct Fixture {
    version: String,
    facts: Vec<FactWrite>,
    decisions: Vec<DecisionWrite>,
    queries: Vec<Query>,
}

fn load() -> Fixture {
    serde_json::from_str(include_str!(
        "../fixtures/memory-retrieval-recall-v1.json"
    )).unwrap()
}
```
Pattern source: `crates/nano-memory/tests/retrieval_recall.rs`.

**Location:**
- Memory acceptance corpus: `crates/nano-memory/fixtures/memory-retrieval-recall-v1.json`.
- Recorded provider/service behavior: `crates/nano-model/fixtures-flux/`.
- Protocol/TUI journeys: `crates/nano-tui/tests/fixtures/*.ndjson`.
- Verifier receipts and source trees: `crates/nano-cli/tests/fixtures/verify/`.
- Redaction positive/negative corpora: `crates/nano-session/fixtures/redaction/`.
- Binary/image adversarial corpus: `crates/nano-tools/fixtures/images/`.
- Sealed gate references and fluent-but-wrong mutants: `gates/fixtures/`.

Fixture rules:
- Commit deterministic fixtures; assert their version/cardinality/seal in tests.
- Use `include_str!` for small immutable contract fixtures so missing files fail at compile time.
- Preserve exact bytes for sealed gate fixtures; `gates/fixtures/.gitattributes` disables text conversion.
- Never place credentials in fixtures. Live Flux credentials remain owner-held and are referenced through established environment/file paths only.

## Coverage

**Requirements:** No line/branch percentage is enforced. Coverage is requirement- and invariant-based through workspace tests, acceptance fixtures, adversarial batteries, platform parity, gate cards, and external evidence.

**View Coverage:**
```bash
# Not defined in repository tooling. Do not invent a coverage threshold.
```

For a new milestone, map every locked acceptance statement to a named test or proof command and record externally verifiable output. `AGENTS.md` explicitly rejects compilation-only completion.

## Test Types

**Unit Tests:**
- Pure parsing, validation, ranking, policy, state transitions, and helpers.
- Usually co-located under `#[cfg(test)]`, or in focused integration files for public contracts.
- Examples: `crates/nano-memory/src/resolver.rs`, `crates/nano-core/src/execrules.rs`.

**Integration Tests:**
- Cross-module public APIs, real files/databases, subprocesses, journaling, CLI, and protocol wiring.
- Examples: `crates/nano-memory/tests/durability.rs`, `crates/nano-cli/tests/verify_cmd.rs`, `crates/nano-mcp/tests/dispatcher_battery.rs`.

**Acceptance and Retrieval Tests:**
- `crates/nano-memory/tests/retrieval_recall.rs` loads the versioned 60-row corpus (50 facts, 10 decisions) and 20 labeled queries.
- It requires recall@10 `>= 0.90` and asserts every hit matches both query project and agent ID; evidence output reports recall and zero cross-project/cross-agent leakage.
- Explicit multi-agent scope is separately tested to ensure it never widens the project boundary.

**Durability and Crash Tests:**
- `crates/nano-memory/tests/durability.rs` spawns the current test executable as a child, blocks after journal sync and before DB commit, kills it, deletes/rebuilds database state from journals, and compares current facts plus retrieval IDs.
- The same file asserts journal operation IDs remain collision-free across reopen/rebuild and unreceipted model writes are ignored.
- Follow this subprocess/marker/kill/rebuild pattern for kill-point claims; a simulated error return is insufficient.

**Write-Authority Tests:**
- `crates/nano-memory/tests/write_mediation.rs` proves direct model-tier writes fail with `MediationRequired`, mediated writes cover every memory kind, receipts are visible and agent-bound, journal operations retain `agent_id`, and secret canaries do not reach the journal.
- Any runtime memory integration must retain these tests and add an end-to-end caller test showing the host, not model output, invokes the commit boundary.

**Adversarial/Property-Style Tests:**
- Iterate exhaustive offsets or shaped corruption inputs with contextual assertions: `crates/nano-session/tests/adversarial_journal.rs`.
- Verify both positive preservation and negative non-invention/non-resurrection properties.
- Policy, egress, filesystem, shell, and image surfaces have adversarial suites under `crates/nano-core/tests/`, `crates/nano-egress/tests/`, and `crates/nano-tools/tests/`.

**UI Tests:**
- Render through ratatui `TestBackend`, compare `insta` snapshots, then assert interaction effects: `crates/nano-tui/tests/l1_snapshots.rs`.
- ACP/TUI journeys replay NDJSON fixtures and use PTY tests for actual terminal behavior: `crates/nano-tui/tests/pty.rs`.

**Gate Cards:**
- `gates/tests/*.test.cjs` validate closed card schemas, registry closure, canonical hashing, atomic writers, fail-closed output parsing, reference trees, and fluent-but-wrong mutants.
- Each card in `gates/<gate-id>/card.md` declares exactly measured checks, tool pins, fixture seals, mutant expectations, gamed modes, and escape-hatch bans.
- A mutant that remains green produces `GATE_DEFECT` and blocks validation; direct gate invocation is authoring evidence only, not dogfood evidence (`gates/README.md`).

**E2E Tests:**
- CLI/agent vertical slices live in `crates/nano-cli/tests/vertical_slice.rs` and related checkpoint suites.
- Live tests are intentionally excluded from normal CI and run through `just gate-live`; they must self-skip without the owner credential.
- Recorded fixtures supply CI replay evidence instead of network calls.

## Common Patterns

**Async Testing:**
```rust
#[tokio::test(flavor = "current_thread")]
async fn behavior_is_deterministic() {
    let backend = MockBackend::new();
    let result = backend.frontmost_app().await.unwrap();
    assert_eq!(result, None);
}
```
Use `current_thread` where concurrency is not under test. Use barriers, explicit hooks, markers, or deterministic clocks for races; do not rely on arbitrary sleep except bounded polling for an external child marker.

**Error Testing:**
```rust
assert!(matches!(
    store.write_fact(model_fact),
    Err(MemoryError::MediationRequired)
));
```
Assert the exact domain variant and also assert prohibited state was not created when the failure is security-relevant.

**Cleanup:**
- Prefer RAII temporary directories.
- For explicit platform resources, validate exact absolute ownership before cleanup and fail if residue remains: `.github/workflows/gate.yml`.
- Never recursively remove an unvalidated computed path or follow reparse points.

## CI and Acceptance Gates

The primary workflow is `.github/workflows/gate.yml`.

**Six native matrix legs:**
- Windows x64: `windows-latest`
- Windows ARM64: `windows-11-arm`
- macOS ARM64: `macos-14`
- macOS x64: `macos-15-intel`
- Linux x64: `ubuntu-22.04`
- Linux ARM64: `ubuntu-24.04-arm`

Every native leg runs pinned Rust 1.95.0 and the substantive gates: format, Clippy with warnings denied, workspace tests, generated-contract drift, and `cargo deny`. Unix legs prebuild sandbox/PTY helpers; Windows isolates the verifier's 14-test inventory onto a dedicated NTFS volume and fails on inventory drift.

**Seventh leg:**
- The separate Windows `gate-cards` job in `.github/workflows/gate.yml` creates an owned F: volume and exact-SHA checkout, builds the landed verifier/helpers, stages authentic artifacts, runs restricted-token gate-card dogfood and meta-tests, checks canaries/ownership/cleanliness, and removes only validated owned resources.

**Additional evidence:**
- Windows x64 generates the canonical CycloneDX SBOM.
- Windows legs run the C2.4 performance profiler as advisory only; performance does not fail the gate.
- Every matrix leg uploads a machine-readable evidence manifest even on failure.
- `.github/workflows/release.yml` is packaging/release automation; do not treat it as a replacement for the PR gate.

**Definition of local green:**
```bash
just gate-all
just gate-deny
```
For a touched crate, run focused tests first, then the workspace gate. For P-MEM changes, also run all three acceptance binaries with `--nocapture` so their evidence is visible.

**Failure discipline:**
- Read the complete failure and fix the root cause; never suppress, skip, or weaken a valid gate.
- A platform-specific red leg is code evidence, not permission to remove the platform.
- Live credential absence is the one expected skip path; do not turn recorded/offline CI into a networked test.

---

*Testing analysis: 2026-08-27*
