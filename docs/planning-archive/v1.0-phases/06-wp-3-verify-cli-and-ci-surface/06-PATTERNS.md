# Phase 6: WP-3 Verify CLI and CI Surface - Pattern Map

**Mapped:** 2026-08-21
**Files classified:** 11 owned files/trees
**Analogs found:** 8 / 11 (three authority-defined artifacts intentionally have no source analog)

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog / Authority | Match Quality |
|---|---|---|---|---|
| `crates/nano-cli/src/verify_cmd.rs` | controller/service/utility | request-response, event-driven, file-I/O | `exec_mode.rs`, `exec_run.rs`, plus `nano_verify` public API | composite role-match |
| `crates/nano-cli/src/lib.rs` | config/module registry | static registration | existing ordered module list in same file | exact |
| `crates/nano-cli/src/main.rs` | controller/route | request-response | existing `exec` arm | exact |
| `crates/nano-cli/Cargo.toml` | config | dependency graph | existing workspace path dependency block | exact |
| `crates/nano-cli/tests/verify_cmd.rs` | test | request-response, event-driven, file-I/O | `c11_exec_process.rs`, nano-verify Git fixture tests | role-match |
| `crates/nano-cli/tests/fixtures/verify/**` | test fixture | file-I/O | runtime Git fixtures in `nano-verify/tests/receipt_git.rs` | role-match |
| `gates/registry.json` | config/model | file-I/O | IFACE schema; no valid production analog | authority-only |
| `docs/verify/VERIFY-CLI.md` | documentation | request-response contract | authoritative WP3 §§2,4,6 | authority-only |
| `docs/verify/CI-ADOPTION.md` | documentation | CI/operator procedure | `.github/workflows/gate.yml`, WP3 §7 | role-match |
| `docs/verify/ci/{verify-receipt-check,verify-dogfood}.yml` | config/CI consumer | event-driven, batch | `.github/workflows/gate.yml`, `.github/workflows/release.yml` | role-match |
| `UPSTREAM.md` | provenance config | append-only ledger | existing provenance table rows | exact |

## Pattern Assignments

### `crates/nano-cli/src/verify_cmd.rs` (controller/service/utility; request-response, event-driven, file-I/O)

This file has no single analog. Reuse the seams below, but import the verification semantics from `nano_verify`; do not copy or recreate them.

**Closed JSONL sink:** `crates/nano-cli/src/exec_mode.rs:95-127`

```rust
pub struct ExecEvents<W: Write> {
    out: W,
    session_id: String,
    seq: u64,
}

fn emit(&mut self, body: serde_json::Value) {
    let mut line = serde_json::json!({
        "v": 1,
        "session_id": self.session_id,
        "seq": self.seq,
    });
    line.as_object_mut().expect("object")
        .extend(body.as_object().expect("object").clone());
    self.seq += 1;
    let mut text = serde_json::to_string(&line).unwrap_or_default();
    text.push('\n');
    let _ = self.out.write_all(text.as_bytes());
    let _ = self.out.flush();
}
```

Copy the envelope/sequence/write discipline, not exec's vocabulary. `VerifyEvents` must expose only WP3's closed frames and fixed error codes. In particular, do not copy `ExecEvents::error`'s free-form behavior (`exec_mode.rs:181-183`) across the verification trust boundary.

**Dependency-injected orchestration seam:** `crates/nano-cli/src/exec_run.rs:25-57`, production wrapper at `:692-694`

```rust
pub async fn run_exec_with<W, FD, FD2, FT, D, T>(/* injected factories and sink */) -> i32
where
    W: Write + Send,
    FD: Fn() -> D,
    FD2: Fn() -> D,
    FT: Fn(&Path, PermissionMode) -> (T, FileSystemSandboxPolicy),
    D: ModelDriver,
    T: ToolExecutor,
{ /* orchestration */ }

pub async fn run(nano_home: &Path, workspace: &Path, params: &ExecParams) -> i32 { /* real wiring */ }
```

Use the same split: a generic-free public `run`, with a private/internal `run_with` holding clock, generation, gate, Git, filesystem, and event seams. The 13-test battery must drive `run_with` without a network key.

**Provider construction (partial analog only):** `crates/nano-cli/src/exec_run.rs:694-807`

```rust
let Some(api_key) = crate::flux_key::flux_api_key() else { return 2; };
let make_driver = move || {
    nano_agent::wiring::FluxDriver::new(
        nano_model::flux_completions::FluxCompletionsClient::new(
            nano_egress::client::EgressClient::new(driver_policy.clone()),
        ),
        api_key.clone(),
    )
};
```

Reuse the established credential resolution and Flux client/driver construction. Model ids, however, come only from the exact CLI cheap/escalation arguments; do not inherit exec auto-routing/default-model logic. The production adapter implements the imported `nano_verify::Effects` seam only. No provider identity, provider error, prompt, or output crosses the event boundary.

**Successful optional output copy:** `crates/nano-cli/src/exec_run.rs:670-689` with `exec_mode::atomic_replace_write` at `exec_mode.rs:409-426`

```rust
if exit == 0
    && let Some(path) = output_last_message
    && let Err(err) = atomic_replace_write(path, final_text.as_bytes())
{
    return 2;
}
```

Apply this success-only pattern to `--receipt-out`. The authoritative receipt store itself must use `nano_verify::write_receipt`, never this helper.

**Exact imported API:** `crates/nano-verify/src/lib.rs:14-38`

```rust
pub use engine::{
    CandidateDiff, ChangeKind, ClimbEventKind, Effects, EngineEvent,
    ExpectedChange, ExpectedChangeManifest, derive_expected_changes,
    parse_candidate_diff, run_climb,
};
pub use gate::{
    ArtifactWorkspace, BaselineGateExecution, CandidateArtifact, CheckVerdict,
    ExecutionGateOutcome, GateInvocation, create_artifact_workspace,
    run_gate_baseline_execution, run_gate_execution,
};
pub use receipt::{
    FailingRun, Receipt, ReceiptPreflight, VerifyVerdict, canonical_receipt,
    mint_receipt, preflight_receipt, read_receipt, write_receipt,
};
pub use registry::{
    CwdPolicy, GateRegistry, GateRegistryEntry, check_closure_pin,
    check_inventory, closure_digest, gate_for_requirement, load_registry,
};
```

These symbols are implementation authorities, not analog suggestions. Import them; do not define local artifact, receipt, registry, digest, parser, hunk, operation, postimage, or verdict equivalents.

**Registry resolution:** `crates/nano-verify/src/registry.rs:92-160`

- Normal populated registries go through `load_registry`.
- Default requirement resolution goes through `gate_for_requirement`.
- Explicit pins still use the loaded registry entry and `check_closure_pin`.
- Inventory comes from `check_inventory` on the selected card.
- `closure_digest` is the only closure digest implementation.

**Empty-bootstrap exception:** `load_registry` deliberately rejects empty maps at `registry.rs:104-107`, while WP3 owns the exact production bootstrap `{"gates":{},"requirements":{},"schema":1}`. Implement only a byte-exact recognizer for that one bootstrap state. It means “no production gates yet”; any requested gate/requirement returns usage exit 2 without scheduled work. Every nonempty registry must pass `load_registry`; do not create an alternate general validator.

**Receipt pipeline:** `crates/nano-verify/src/receipt.rs:15-76,457-502`

Use the imported `Receipt`/`ReceiptPreflight` types and locked preflight. `write_receipt` already supplies canonical bytes, exclusive locking, same-directory temporary storage, sync, and platform atomic replacement. WP3 adds only the post-`Ready` detached worktree rerun and maps Green→`Valid`, Red→`GateMismatch`, and worktree/probe/timeout failure→`Unverifiable`.

**Candidate materializer:** `crates/nano-verify/src/engine.rs:19-73` plus IFACE §5A

Call `CandidateArtifact::read_exact_bytes`, then exactly one `parse_candidate_diff` and exactly one `derive_expected_changes`. Retain `ExpectedChangeManifest::{entries,base_tree_digest,diff_digest}` through staged and committed verification. Git output observes actual state; it never becomes an alternate expected-change oracle. Use argv/stdin only for the exact `git apply --check --index --whitespace=error-all -` then `git apply --index --whitespace=error-all -` sequence.

**Git subprocess hygiene:** `crates/nano-verify/src/receipt.rs:95-123` and `crates/nano-checkpoints/src/lib.rs:368` are the nearest product analogs. Use `Command` argv, explicit cwd, bounded execution, scrubbed environment, and closed result classification; never shell-concatenate candidate bytes or emit Git stderr.

**Detached-worktree cleanup:** no reusable Rust product guard exists. Implement an owned cleanup guard in `verify_cmd.rs` following the lifecycle proven in phase closure plans: add detached worktree; on every path run `git worktree remove --force`, then `git worktree prune`; verify both filesystem absence and absence from `git worktree list --porcelain`. Cleanup failure overrides the ordinary result fail-closed. Never use raw recursive deletion as the primary cleanup mechanism.

### `crates/nano-cli/src/main.rs` (controller/route; request-response)

**Analog:** `crates/nano-cli/src/main.rs:59-74`

```rust
Some("exec") => {
    let home = nano_home();
    let workspace = std::env::current_dir().expect("cwd");
    match parse_exec_args(&args[2..]) {
        Err(code) => code,
        Ok(params) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().expect("tokio runtime");
            runtime.block_on(nano_cli::exec_run::run(&home, &workspace, &params))
        }
    }
}
```

Insert the spec's verify arm before `--version`; delegate parsing to `verify_cmd::parse_args`, then pass the exit code through unchanged. Update only the fallback usage string at `main.rs:129-134`.

### `crates/nano-cli/src/lib.rs` and `Cargo.toml` (config)

- Module ordering analog: `lib.rs:5-24`; add `pub mod verify_cmd;` immediately after `shell_rules` as locked by the WP3 spec.
- Workspace dependency analog: `Cargo.toml:31-49`; add `nano-verify = { version = "0.1.0", path = "../nano-verify" }` immediately after `nano-tools`.
- Do not add a new external package. Existing serde, serde_json, sha2, tokio, tempfile, and platform dependencies cover the authorized implementation.

### `crates/nano-cli/tests/verify_cmd.rs` and fixtures (test; request-response/event/file-I/O)

**Process harness:** `crates/nano-cli/tests/c11_exec_process.rs:27-72,82-99`

```rust
let output = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
    .args([/* closed argv */])
    .env("NANO_HOME", &dir)
    .current_dir(&dir)
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .output()
    .expect("spawn wayland-nano");
let lines: Vec<serde_json::Value> = BufReader::new(&output.stdout[..])
    .lines().map(|line| serde_json::from_str(&line.unwrap()).unwrap()).collect();
for (index, event) in lines.iter().enumerate() {
    assert_eq!(event["v"], 1);
    assert_eq!(event["seq"].as_u64().unwrap(), index as u64);
}
```

Reuse binary discovery, explicit cwd/env, piped output, JSON-line parsing, and monotonic sequence assertions. Unlike the live exec test, all verify tests are keyless/offline and must never self-skip.

**Runtime Git fixture:** use content-only checked-in fixtures and create `.git` at runtime, following `crates/nano-verify/tests/receipt_git.rs:1-105`: initialize, configure local author identity, author commit A, author fix commit B, and capture SHAs with argv-only Git. Compute fixture closure digests by calling `nano_verify::closure_digest`; never embed a parallel digest algorithm.

**Temp/target discipline:** fixture repositories remain under a private canonical F: temporary root. Nested Cargo runs should derive a unique short target directory from the outer `CARGO_TARGET_DIR` (or a short repository `target` fallback), hold any global environment lock for env mutation, and clean that target before releasing the lock. This avoids Windows linker path failures while preserving private fixture source.

**Exact oracle:** define the 13 names verbatim from WP3 §8 and ensure each is listed once. Tests 1–2 use `run_with` scripted effects; tests 3–13 are process-level. Add explicit teeth for event leakage, deletion/rename CI handling, protected-path overlap, rollback, exact deadline narrowing, worktree cleanup, and the bootstrap/nonempty registry split.

### `gates/registry.json` (config/model; file-I/O)

No codebase analog should be copied. Create exactly these bytes, with no trailing newline:

```json
{"gates":{},"requirements":{},"schema":1}
```

WP3 must not add any production entry.

### `docs/verify/**` and CI YAML (documentation/config; batch)

**Workflow conventions:** `.github/workflows/gate.yml:19-45` and `.github/workflows/release.yml:12-18,53-64`

- Use a `pull_request` trigger, minimal `permissions: contents: read`, pinned action majors, and `fetch-depth: 0` for ancestry proof.
- Keep authored consumers under `docs/verify/ci/**`; WP3 never promotes `.github/**`.
- Pin the verifier package version exactly; never use `@latest`.
- Receipt selection must consume `git diff --name-status`; `D*|R*` fails, only `A*|M*` verifies. Every nonzero CLI exit fails the job.
- `CI-ADOPTION.md` records that owner promotion and required-status-check configuration are deferred until after WP-4 sealed mutants land. `VERIFY-CLI.md` records exact argv/exits/events and explicitly says a well-formed changed `log_digest` cannot be detected offline: it is provenance, not independently recomputed proof.

### `UPSTREAM.md` (provenance config; append-only)

**Analog:** existing table entries such as `UPSTREAM.md:190`.

Add one row naming each WP3 destination and the exact donors: `.tmp/upstream-ferrox-factory/src/strength-receipt.cts` plus `gates/README.md`. Describe semantic transformations: standalone IFACE receipt, no skip-on-unverifiable, locked preflight plus detached rerun, identifiers-only event boundary, and docs-owned CI consumers. Do not claim verbatim copying where none occurred.

## Shared Patterns

### Closed failure boundary

Apply to all verify orchestration: fixed error/event codes only; no `VerifyError` display/debug, provider text, Git stderr, path, argv, source, expected value, log, prompt, or diff in stdout/stderr frames.

### Absolute deadline

Construct one `RunDeadline` with checked monotonic-millisecond addition before any scheduled effect. Before each provider/artifact/gate/Git/worktree/store operation, resample the same clock. Narrow gate timeout to the exact checked remainder; zero/expired/overflow starts nothing. Receipt verification uses only its separately capped environment budget.

### Git transaction and cleanup

Capture source HEAD/tree/clean status, run baseline in a detached start-commit worktree, re-prove both checkouts, apply once to index, verify against the sealed manifest, commit once, rerun, then mint. Before commit, every failure rolls back and proves exact restoration; after commit, never rewrite history. Every detached worktree has unconditional remove/prune/absence verification.

### Ownership fence

No edits to `crates/nano-verify/**`, `.github/**`, production Gate Cards, `docs/verify/gates.md`, owner status files, or shared authorities. The owned list in WP3 §1 is exhaustive.

## No Analog Found

| File / Concern | Reason | Planner Direction |
|---|---|---|
| `gates/registry.json` empty bootstrap | Landed loader intentionally rejects empty registries | Use only the authority's exact bytes and narrow bootstrap recognizer |
| Trusted materializer in `verify_cmd.rs` | No existing installer has the sealed-manifest trust contract | Implement directly from IFACE §5A using imported parser/manifest APIs |
| Detached receipt-rerun worktree guard | No reusable Rust product guard exists | Implement owned RAII/lifecycle guard with remove→prune→absence proof |

## Metadata

**Analog search scope:** `crates/nano-cli`, `crates/nano-verify`, `crates/nano-checkpoints`, `.github/workflows`, `docs`, `UPSTREAM.md`, Phase 6 inputs, and frozen external WP3/interface authorities.

**Strong analogs read:** `main.rs`, `lib.rs`, `Cargo.toml`, `exec_mode.rs`, `exec_run.rs`, `provider_router.rs`, `flux_key.rs`, `c11_exec_process.rs`, `nano-verify/{lib,registry,receipt,climb,engine}.rs`, workflow files, and provenance ledger.

**Pattern extraction date:** 2026-08-21
