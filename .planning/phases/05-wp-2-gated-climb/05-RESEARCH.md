# Phase 5: WP-2 Gated Climb - Research

**Researched:** 2026-08-20
**Domain:** Rust ratcheted state machine, sealed artifact execution, strict unified-diff parsing
**Confidence:** HIGH — the phase is governed by hash-frozen internal authorities and an already-landed WP-1 API.

## Summary

WP-2 should be planned as three cooperating but sharply separated responsibilities: a pure climb state machine in `climb.rs`, a trusted async driver plus the single candidate-diff/expected-change implementation in `engine.rs`, and a bounded evidence/artifact amendment to the landed runner in `gate.rs`. The decision core never performs I/O; the driver alone calls the injected generation/time/cancellation/event seam; trusted `nano-verify` core, not `Effects`, creates and validates candidate artifacts and launches gates. [VERIFIED: `SPEC-WP12-nano-verify-engine.md` §§3.1–3.6; `SPEC-WP-INTERFACES.md` §5]

The acceptance rule is deliberately narrower than a score/failure-count heuristic: accept a candidate only when its passed score is strictly greater, or at equal score when its deduplicated canonical failure set is a strict subset. Equal-count substitutions such as `{A,B}` → `{C,D}` must reject, eliminating oscillation. The phase sequence is probe → budget-truncated sequential ensemble → per-check cheap then ladder escalation → at most one consolidation → typed stop. [VERIFIED: `SPEC-WP12-nano-verify-engine.md` §3.1]

The largest planning risk is underestimating the trust-boundary work embedded in test 33. WP-2 must add sealed OS-temp artifacts, complete-output evidence semantics, deadline/cancellation ownership, a complete schema-1 unified-diff parser, and read-only expected-change derivation while preserving the exact landed `run_gate(inv, artifact_path, inventory)` compatibility API. It must not apply a patch, mutate Git, capture baseline red evidence in the climb, mint receipts, add provider defaults, or begin WP-3/WP-4/expansion work. [VERIFIED: `docs/FOLLOWUPS.md` DEV-WP-2A; `GOALS.md` WP-2; `SPEC-WP12-nano-verify-engine.md` §§5–8]

**Primary recommendation:** Plan in dependency order: freeze public types and pure ratchet tests; implement sealed workspace/candidate and evidence-bearing runner; implement parser plus manifest derivation; then connect the async driver and close the complete named test/audit/gate battery. [VERIFIED: current crate seams plus authoritative test ownership in `SPEC-WP12-nano-verify-engine.md` §7]

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|---|---|---|---|
| Ratchet, phase transitions, tried/wins memory | Pure decision core (`climb.rs`) | — | `next_step`, `apply_result`, and `better_candidate` must remain deterministic and I/O-free. [VERIFIED: WP12 §3.2] |
| Generation, clocks, cancellation, events | Injected driver (`engine.rs` + `Effects`) | Pure core | The driver schedules effects and folds closed `StepResult`s into pure state. [VERIFIED: IFACE §5; WP12 §3.4] |
| Candidate workspace, byte identity, gate process | Trusted verification core (`gate.rs`) | Driver | Callers cannot choose paths or forge artifacts/evidence; core launches and hashes. [VERIFIED: IFACE §5] |
| Candidate grammar and expected-change manifest | Trusted parser/materializer model (`engine.rs`) | WP-3 consumer | WP-2 owns the sole parser and derivation; WP-3 later consumes them once after climb. [VERIFIED: IFACE §5/§5A; GOALS WP-2] |
| Repository install, Git commit, receipt mint | WP-3 (out of scope) | — | WP-2 returns only an opaque accepted artifact and never mutates a repository. [VERIFIED: DEV-WP-2A; IFACE §5A] |

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|---|---|---|
| CLIMB-01 | Probe, ensemble, surgical escalation, consolidation through `Effects`. | Exact state transitions, ordering, and ownership are pinned below. [VERIFIED: REQUIREMENTS.md; WP12 §3.1] |
| CLIMB-02 | Strict-score or strict-subset acceptance only. | The pure acceptance invariant and oscillation oracle are pinned below. [VERIFIED: REQUIREMENTS.md; WP12 §3.1] |
| CLIMB-03 | Expose only opaque canonical failing-check identifiers. | Prompt/event/log allowlists and leakage canaries are pinned below. [VERIFIED: REQUIREMENTS.md; WP12 §3.5] |
| CLIMB-04 | Default budget 12, every call charged, typed escalation and complete exits. | Budget semantics, validation, deadlines, and terminal mapping are pinned below. [VERIFIED: REQUIREMENTS.md; WP12 §§3.1–3.4] |
| CLIMB-05 | Complete driver-stub regression battery. | Exact public tests 30–33 and required sub-assertions are mapped under Validation Architecture. [VERIFIED: REQUIREMENTS.md; WP12 §7] |
</phase_requirements>

## Frozen Authority and Scope

`docs/FOLLOWUPS.md` DEV-WP-2A records the owner freeze after WP-1 promotion: `GOALS.md` is 20,917 bytes / SHA-256 `299234b769ab0cdbd91207ed82f512f28d1cbc962d196317e526205f790d3b62`; IFACE is 56,256 bytes / `57c147b970094a83650ea95ca69fb586b63a70e586bdc140bc70f8ab1de48a4d`; WP12 is 67,006 bytes / `8ae69bb42f492a26a4f8512775947a91e1c6c3ba541797756d1d53634cc5d491`. The hashes were independently reproduced during this research. [VERIFIED: `docs/FOLLOWUPS.md`; PowerShell `Get-FileHash` on F: authorities]

The WP-2 product ownership set is `crates/nano-verify/**`, exactly two new `UPSTREAM.md` rows (`climb.rs`, `engine.rs`), and a bounded amendment to the existing `gate.rs` provenance row. Root `Cargo.toml` registration is already landed and is a no-op. No other crate, generated error table, `.github/**`, Gate Card, CLI, WP-5/WP-6, DeepSeek harness, profiles, memory, MCP, or external-agent expansion surface is granted. [VERIFIED: `GOALS.md` WP-2; WP12 §§5.3, 6, 8; DEV-WP-2A]

## Project Constraints (from AGENTS.md)

- Work only in this repository (and shared only when explicitly required); `../nano/` and `../resources/upstreams/` are read-only. [VERIFIED: `AGENTS.md`]
- Never read or expose the Flux key value; only its path may be referenced. [VERIFIED: `AGENTS.md`]
- Fail closed; never weaken sandbox, egress, policy, journal, runner, or tests to obtain green. [VERIFIED: `AGENTS.md`]
- Rust is pinned to 1.95.0, edition 2024; `windows-sys` remains 0.52 with no second version. [VERIFIED: `AGENTS.md`; `rust-toolchain.toml`; `Cargo.toml`]
- Completion requires `just gate-all` (fmt, clippy `-D warnings`, workspace tests, generator check) and externally inspected evidence. [VERIFIED: `AGENTS.md`; `justfile`]
- Donor adaptations require file-specific `UPSTREAM.md` provenance; generated tables are generator-owned. [VERIFIED: `AGENTS.md`]
- Several agents share the tree; edits stay within assigned ownership. This researcher owns only this RESEARCH.md and must not commit without explicit owner authorization. [VERIFIED: assignment; `AGENTS.md`]

## Standard Stack

### Core

| Library/tool | Landed version/pin | Purpose | Planning disposition |
|---|---:|---|---|
| Rust | 1.95.0, edition 2024 | All implementation | Use existing toolchain unchanged. [VERIFIED: `rust-toolchain.toml`, workspace manifest] |
| Tokio | workspace-resolved 1.x; features `io-util,macros,process,rt,time` | Process execution, async timers/select, tests | Reuse the landed feature union; no async-trait crate. [VERIFIED: `crates/nano-verify/Cargo.toml`; IFACE §5] |
| serde / serde_json | landed 1.x | Closed serialized types and canonical structures | Reuse; preserve exact derives and deny-unknown behavior where specified. [VERIFIED: crate manifest; IFACE conventions/§8] |
| sha2 | landed 0.10 | Candidate, stdout, postimage, diff, and tree digests | Reuse existing dependency. [VERIFIED: crate manifest; IFACE §5] |
| tempfile | landed 3.x | Private fresh OS-temp workspace | Reuse only behind the zero-argument core factory; caller paths remain forbidden. [VERIFIED: crate manifest; IFACE §5] |
| windows-sys | exact policy pin 0.52 | Windows process containment and reparse/file primitives | Do not bump or add another version; broaden features only if the frozen API demonstrably requires it and ownership permits it. [VERIFIED: `AGENTS.md`; crate manifest] |

No external package installation is required by the frozen design, so a package-legitimacy audit is not applicable. Cargo lock/dependency inspection remains a phase gate and should demonstrate zero new third-party crates unless an owner-recorded deviation is approved. [VERIFIED: WP12 §8]

## Recommended Project Structure

```text
crates/nano-verify/
├── src/
│   ├── climb.rs       # pure state, ratchet, typed outcome construction
│   ├── engine.rs      # parser, manifest derivation, Effects driver, prompts
│   ├── gate.rs        # landed runner + additive sealed/evidence APIs
│   ├── lib.rs         # exact public re-exports
│   └── error.rs       # existing crate-local errors; no NanoErrorKind
├── tests/
│   ├── gate_contract.rs  # compatibility + evidence subprocess fixtures
│   └── ...               # external compile-fail/API boundary fixture as planned
└── Cargo.toml            # existing dependencies/features unless justified
UPSTREAM.md               # exactly two new rows + bounded gate-row amendment
```

[VERIFIED: WP12 §§4–5.3, 7–8]

## Architecture Patterns

### Primary Flow

```text
caller config + spec + inventory + GateInvocation + opaque ArtifactWorkspace
        |
        v
validate config/workspace/deadline ----invalid----> sealed error outcome + Stopped
        |
        v
pure next_step (probe / ensemble / surgical / consolidate / stop)
        |
        v
Effects.generate(model, sanitized prompt) --error--> charged GenerationFailed
        |
        v
parse_candidate_diff(exact UTF-8 bytes) --invalid--> charged reject, no file/gate
        |
        v
core-created CandidateArtifact -> run_gate_execution -> sealed evidence/outcome
        |
        v
pure apply_result / strict ratchet -> repeat or typed ClimbOutcome
        |
        +--> accepted opaque artifact only (WP-3 later reads/parses/derives/applies)
```

[VERIFIED: IFACE §5; WP12 §§3.1–3.5]

### Pattern 1: Pure Functional State Fold

`next_step(&ClimbState)` selects one closed action and `apply_result` returns a new state without mutating inputs. Calls are the count of attempted model generations; generation failures consume calls but never produce artifacts/evidence or seed `best`. Wins increment only on ratchet acceptance. Tried memory is keyed by the full canonical failure string, persists across ordinary accepts but is pruned to still-failing keys, and resets only after an accepted consolidation. [VERIFIED: WP12 §§3.1–3.2]

The stable ordering contract is material: cheap surgical candidates sort by wins descending with caller order as tie-break; ladder order is always caller order and never win-sorted. Ensemble execution is sequential and truncated before scheduling to remaining budget. [VERIFIED: WP12 §3.1]

### Pattern 2: Opaque Core-Created Artifact Identity

`create_artifact_workspace()` accepts no path, derives its canonical parent from `std::env::temp_dir()`, creates a private child, and returns a non-Clone opaque guard. `create_candidate_artifact` is crate-private and binds workspace identity, private path, exact bytes, and SHA-256. `CandidateArtifact::Debug` omits paths; readback repeats confinement and digest validation. The accepted handle keeps the workspace alive until the last handle drops. [VERIFIED: IFACE §5]

Executor preflight, not product code, must prove canonical `TEMP` and `TMP` beneath `F:\Temp\Codex`. Product code must never hard-code F: or accept a caller temp root. [VERIFIED: IFACE §5; assignment constraints]

### Pattern 3: Compatibility Wrapper over a Shared Launcher

Preserve `run_gate(&GateInvocation, &Path, inventory) -> GateOutcome` source and behavior. Add `run_gate_execution` for sealed candidate evidence and `run_gate_baseline_execution` for later WP-3 baseline use. A private launcher may be shared, but policies differ: legacy `run_gate` truncates stdout at 16 MiB and parses the prefix; evidence APIs require complete capture up to 16 MiB inclusive and return `OutputIncomplete` with no digest on overflow. [VERIFIED: IFACE §4–§5]

Evidence is core-derived: normal exit yields `Some(exit_code)` even when nonzero; complete stdout yields `Some(log_digest)`; candidate execution additionally always binds exact `artifact_sha256`. Spawn, timeout, abnormal/no-code termination, or incomplete output use `None` for ineligible evidence fields and typed fail-closed outcomes. Stderr is discarded. [VERIFIED: IFACE §5]

### Pattern 4: Single Strict Parser and Sealed Manifest

`parse_candidate_diff` is the only WP-2/WP-3 grammar implementation. It accepts only nonempty ≤16 MiB UTF-8, LF-only raw unified diffs with exact `diff --git`, OLD/NEW, and hunk structure; ASCII confined unique paths; Add/Modify/Delete header pairs; checked `u64` ranges; exact body counts; and fully consumed input. It rejects prose/fences, CR/NUL, extended headers, binary/mode/link/rename/copy forms, quoted/escaped paths, malformed/trailing data, duplicate paths, and unsupported newline markers. [VERIFIED: IFACE §5]

`derive_expected_changes` consumes only `CandidateDiff` private parsed records, never reparses bytes or asks Git to interpret the patch. It descriptor-confines a canonical detached root, rejects `.git`/links/reparse/mount/volume escape/nonregular objects, reconstructs Add/Modify/Delete postimages in memory, and returns a sorted immutable manifest binding exact diff digest, touched-preimage base-tree digest, and Add/Modify postimage digests. It makes no filesystem or Git mutation. `run_climb` neither calls it nor carries its result. [VERIFIED: IFACE §5; WP12 §§3.2, 3.4]

### Pattern 5: Engine-Owned Deadlines and Cancellation

Validate budget/model pool/workspace/deadline before generation. Each provider call is capped at `min(now.checked_add(120_000), run_deadline)` and driven by the core against a millisecond timer plus cancellation polls no slower than 50 ms. The losing generation future is dropped, never detached. Check cancellation before generation, after selection, before artifact creation, and before gate execution. [VERIFIED: IFACE §5]

Before each gate, recompute remaining whole milliseconds; checked-convert `gate.timeout.as_millis()` to `u64`; use the nonzero minimum; never round a sub-millisecond timeout upward. Overflow maps to `Blocked("deadline_overflow")/Error`; expiry maps to `TimedOut/Error`; cancellation takes the frozen precedence where asserted by test 33. [VERIFIED: IFACE §5; WP12 §7 test 33]

## Public Types and Terminal Mapping

Import `Phase`, `Tier`, `StopReason`, `LogCode`, `LogEntry`, `TerminalState`, `ClimbOutcome`, `RunDeadline`, `EngineEvent`, `ClimbEventKind`, and `Effects` exactly from the frozen IFACE shapes. All `ClimbOutcome` fields are private with read-only accessors and a private seal; `accepted_artifact` and its seal are serde-skipped, and debug output exposes only semantic state plus a boolean presence marker. External literal construction and mutation must fail to compile. [VERIFIED: IFACE §§5, 8]

| Condition | Terminal | StopReason |
|---|---|---|
| Green solution | `Verified` | `Solved` |
| Budget exhausted / second plateau | `NeedsEscalation` | `Budget` / `Plateau` |
| Empty cheap pool / no seed | `Blocked("no cheap models configured")` | `Exhausted` |
| Zero budget | `Blocked("zero_budget")` | `Error` |
| Blank/duplicate model ids | `Blocked("invalid_model_pool")` | `Error` |
| Checked time arithmetic overflow | `Blocked("deadline_overflow")` | `Error` |
| Deadline expiration | `TimedOut` | `Error` |
| Cancellation | `Cancelled` | `Error` |
| Unsafe/missing workspace | `PermissionDenied` | `Error` |

[VERIFIED: IFACE §5; WP12 §§3.3–3.4]

`CriteriaChecked`, `SelfChecked`, `CrashedRecovered`, and `Superseded` exist for enum completeness but WP-2 never emits them; `is_verified()` is true only for `Verified`. [VERIFIED: IFACE §8; WP12 §§3.3, 7]

## Trust-Boundary Allowlist

Prompts may receive only the caller spec, bounded current diff, and opaque canonical check identifiers. Surgical prompts carry target plus at most eight other IDs; consolidation carries at most ten. Every prompt requires raw UTF-8 unified-diff output with no fences/prose and ≤16 MiB. [VERIFIED: WP12 §3.5]

`Effects` receives only `(model, prompt)` and closed engine events. Events/logs/outcomes must never expose model/provider IDs, provider error text, gate argv/path/source, inventory categories beyond supplied IDs, expected values, raw stdout/stderr, artifact paths/bytes/digests where not explicitly allowed, or free-form notes. `EngineEvent.check_ids` is bounded to the supplied inventory. [VERIFIED: IFACE §5; WP12 §§3.4–3.5]

Model identifiers are opaque caller configuration only. `nano-verify` ships no built-in model/provider IDs and discovers none from environment/global state. Empty cheap is a valid deterministic Exhausted outcome; blank or duplicate IDs across cheap+ladder are invalid before any effect. [VERIFIED: WP12 §§3.4, 3.6]

## Don't Hand-Roll

| Problem | Do not build | Use instead | Reason |
|---|---|---|---|
| Async trait erasure | `async-trait` dependency | Rust 1.95 async fn in trait with generic `E: Effects` | Frozen seam explicitly excludes async-trait. [VERIFIED: IFACE §5] |
| Patch interpretation | Git parser, `git apply --check`, second parser | `parse_candidate_diff` private parsed records | WP-2/WP-3 require one grammar authority. [VERIFIED: IFACE §5] |
| Expected change discovery | `git status`/diff output inference | `derive_expected_changes` | Manifest must bind preimages/postimages without mutation. [VERIFIED: IFACE §5] |
| Candidate path selection | Caller directory/path adapter | zero-argument `create_artifact_workspace` | Prevents artifact substitution and path escape. [VERIFIED: IFACE §5] |
| Gate result injection | Adapter-supplied outcome/digest | core `run_gate_execution` | Evidence and verdicts must bind executed bytes and inventory. [VERIFIED: IFACE §5] |
| Failure ranking | count/severity heuristic | strict score-or-subset ratchet | Count allows oscillation; severity is not in the protocol. [VERIFIED: WP12 §3.1] |
| Provider timeout delegation | Provider-specific cancellation/error variants | core timer + cancellation poll | Engine owns deterministic deadline enforcement. [VERIFIED: IFACE §5] |

## Common Pitfalls

### Pitfall 1: Treating Test 33 as “one driver happy path”

It is one named `#[tokio::test]` but owns extensive subcases: model-pool validation, deadlines, cancellation/future-drop, generation failures, leakage, parser grammar, workspace attacks, evidence overflow/abnormal termination, compatibility mapping, inventory coherence, compile-fail sealing, manifest derivation, and runtime API boundaries. Plan it as multiple implementation tasks with focused helper tests while preserving exactly one public test name. [VERIFIED: WP12 §7 test 33]

### Pitfall 2: Compiling `engine.rs` only at final export

The module must be registered in `lib.rs` when its first implementation task lands or its code/tests will not compile. Public re-exports can be finalized later, but module inclusion is an early dependency. [VERIFIED: current `lib.rs` has no engine module; Rust module reachability]

### Pitfall 3: Accidentally changing landed WP-1 behavior

`FailClosedReason::InconsistentSummary` and legacy runner overflow/mapping remain exact. New detailed reasons live in `ExecutionFailClosedReason`; `InconsistentVerdicts` maps back to legacy `InconsistentSummary`, and abnormal termination maps to legacy `NoGateOutput`. [VERIFIED: IFACE §4–§5]

### Pitfall 4: Letting malformed generation reach disk or a gate

Parsing precedes artifact creation. Invalid/prose/fenced/oversize output consumes one model call, creates no artifact, invokes no gate, and cannot replace `best`. [VERIFIED: WP12 §3.4]

### Pitfall 5: Confusing artifact validation with repository authorization

WP-2 validates structural unified-diff paths but defines no public protected-path predicate and never installs. WP-3 later owns fixed-argv apply, protected target rules, Git state, and receipt minting. [VERIFIED: IFACE §5A; DEV-WP-2A]

### Pitfall 6: Unsafe filesystem checks based only on canonical path strings

The contract requires repeated identity/confinement validation, including link/junction/reparse/mount and alternate-volume attacks, not a single `canonicalize` prefix check. Manifest traversal must be descriptor-relative where specified. [VERIFIED: IFACE §5]

### Pitfall 7: Timer arithmetic that silently extends authority

Saturating/wrapping math, seconds conversion, unchecked casts, stale-now arithmetic, or rounding sub-millisecond timeouts up violate the frozen deadline contract. [VERIFIED: IFACE §5]

### Pitfall 8: Leaking diagnostics through convenient derives

Avoid derived Debug on opaque workspace internals and artifact paths; do not put prompt/provider errors/models/raw logs in public state. Compile-fail and serialization/debug leakage tests are required teeth. [VERIFIED: IFACE §§5, 8; WP12 §7]

## Validation Architecture

### Test Framework

| Property | Value |
|---|---|
| Framework | Rust built-in test harness + Tokio test macro, pinned workspace toolchain |
| Config | `crates/nano-verify/Cargo.toml`, workspace `Cargo.toml`, `rust-toolchain.toml` |
| Quick run | `cargo test -p nano-verify <exact_test_name> -- --exact --nocapture` |
| Crate run | `cargo test -p nano-verify` |
| Dependency gate | `cargo deny check` |
| Phase gate | `just gate-all` |

[VERIFIED: existing Phase 04 plans/gates; WP12 §8]

### Exact Named WP-2 Tests 30–33

| # | Exact name | Owner | Required behavior |
|---:|---|---|---|
| 30 | `ratchet_accepts_strict_score_win_and_strict_subset_only` | `climb.rs` unit | Score win, strict subset, and equal-count oscillation rejection. [VERIFIED: WP12 §7] |
| 31 | `probe_ensemble_surgical_consolidate_path` | `climb.rs` unit | Exact phase sequence, budget truncation, cheap/ladder ordering, tried prune/reset, accept/reject and winner identity. [VERIFIED: WP12 §7] |
| 32 | `budget_exhaustion_stops` | `climb.rs` unit | Budget 3 causes exactly three calls; always-red stops Budget; wins count accepts only. [VERIFIED: WP12 §7] |
| 33 | `driver_stub_suite` | `engine.rs` Tokio test | Full driver/trust/parser/artifact/evidence/manifest/terminal/compile-boundary suite described below. [VERIFIED: WP12 §7] |

Test 33 must include: one-call green; caller ordering and invalid pools; typed deadline arithmetic/entry/between-operation expiry; sub-ms floor and gate-timeout cap; cancellation precedence; never-resolving generation bounded by timer/poll with losing future dropped; charged sanitized generation errors; prompt/event/log leakage canaries; exhaustive schema-1 positive/negative parser cases including exact size boundary; invalid-generation no-persist/no-spawn; accepted byte/digest readback; every terminal and `is_verified`; workspace lifetime/opacity and traversal/link/reparse/volume/move/substitution attacks; candidate mutation/readback failure; complete-output digest and 16 MiB overflow; stderr exclusion; abnormal/spawn/timeout evidence; exact inventory coherence and four-field `InconsistentVerdicts` from both new execution APIs; legacy wrapper compatibility; external literal/mutation compile failures; sorted Add/Modify/Delete manifest, multi-hunk preservation, stable digests, unsafe/missing/preimage-change negatives, no-mutation snapshot, parser reuse, and proof that `run_climb` carries no starting root/manifest. [VERIFIED: WP12 §7 test 33]

Helper tests may be added for implementation clarity, but named inventory checks must find tests 30–33 exactly once and must not rename or split the authoritative public names. [VERIFIED: WP12 §§7–8]

### Requirements → Test Map

| Requirement | Fast evidence | Phase evidence |
|---|---|---|
| CLIMB-01 | tests 31, 33 | full crate + gate-all |
| CLIMB-02 | test 30 | full crate + gate-all |
| CLIMB-03 | test 33 leakage subcases | scoped diff/canary + gate-all |
| CLIMB-04 | tests 32, 33 | full crate + gate-all |
| CLIMB-05 | exact inventory 30–33 | exact named/full/gate battery |

[VERIFIED: REQUIREMENTS.md; WP12 §§7–8]

### Sampling Rate and Wave 0

- Per implementation task: run the exact affected named/helper tests and `cargo clippy -p nano-verify --all-targets -- -D warnings`. [VERIFIED: AGENTS gate discipline]
- Per wave: run `cargo test -p nano-verify`; when runner/process behavior changes, run the relevant subprocess tests repeatedly on the native host. [VERIFIED: prior WP-1 CI portability history and phase gate]
- Phase: enumerate all 33 authoritative names exactly once, run full crate, `cargo deny check`, inspect lock/features, then `just gate-all`. [VERIFIED: WP12 §8]
- Wave 0 gaps: `climb.rs`, `engine.rs`, their module registration/re-exports, external compile-boundary fixture, and WP-2 test names 30–33 are absent and must be created. Existing WP-1 tests 1–29 must remain green. [VERIFIED: `rg --files crates/nano-verify`; Phase 04 verification]

## Security Domain

### Applicable ASVS Categories

| Category | Applies | Control |
|---|---|---|
| V2 Authentication | no | No identity/authentication surface in this crate. [VERIFIED: phase scope] |
| V3 Session Management | no | No session state. [VERIFIED: phase scope] |
| V4 Access Control | yes, local trust boundary | Private seals, crate-private constructors, opaque workspace ownership, exact API allowlists. [VERIFIED: IFACE §5/§8] |
| V5 Input Validation | yes | Strict complete parser, model-pool validation, inventory coherence, checked numeric/time arithmetic. [VERIFIED: IFACE §5] |
| V6 Cryptography | yes | Existing SHA-256 library only; no custom cryptography. [VERIFIED: crate manifest; IFACE §5] |
| V12 Files and Resources | yes | OS-temp confinement, repeated identity checks, bounded bytes, descriptor-relative traversal. [VERIFIED: IFACE §5] |
| V13 API | yes | Closed enums/events, private construction, no adapter-provided evidence. [VERIFIED: IFACE §§5, 8] |

### Threat Register for Planning

| Threat | STRIDE | Required mitigation/test |
|---|---|---|
| Forged candidate/outcome/evidence | Spoofing/Tampering | Private seals/fields, crate-private candidate constructor, compile-fail external construction/mutation. [VERIFIED: IFACE §§5,8] |
| Candidate path/byte substitution | Tampering | Opaque workspace identity, repeated confinement and SHA binding, move/link/reparse/volume attack tests. [VERIFIED: IFACE §5] |
| Parser ambiguity or installer divergence | Tampering | One strict parser; derivation consumes private parsed records; no Git/second grammar. [VERIFIED: IFACE §5] |
| Incomplete/forged gate evidence | Repudiation/Tampering | Core launcher, complete-output policy, typed incomplete/abnormal outcomes, exact inventory coherence. [VERIFIED: IFACE §5] |
| Gate/provider/secret leakage | Information Disclosure | Minimal `Effects`, bounded closed logs/events, scrubbed gate environment, prompt/event canaries. [VERIFIED: AGENTS; WP12 §3.5] |
| Hung generation or gate | Denial of Service | 120s provider cap, run deadline, ≤50ms cancellation polling, dropped future, process-tree timeout. [VERIFIED: IFACE §5; landed WP-1 runner] |
| Ratchet oscillation / budget bypass | Denial of Service | strict subset logic, pre-truncated ensemble, charge every generation attempt, no call after stop. [VERIFIED: WP12 §3.1] |
| Workspace/root escape | Elevation of Privilege | no caller root, reject unsafe temp/root/components/alternate volume, descriptor-relative manifest traversal. [VERIFIED: IFACE §5] |

The bounded Critical/High audit should focus on these eight threat groups plus test weakening, API leakage, legacy runner regression, portability, dependency drift, and scope/provenance. Exactly one audit round and at most one fix round are authorized. [VERIFIED: V3 standing rule 6; ROADMAP Phase 5 promotion gate]

## Environment Availability

| Dependency | Required By | Available | Version/constraint | Fallback |
|---|---|---:|---|---|
| Rust/Cargo | build/test | yes | pinned 1.95.0 | none |
| F: temp root | artifact tests | required preflight | canonical beneath `F:\Temp\Codex` | abort; do not fall back to C: |
| F: cargo target | builds | configured operating constraint | `F:\CargoTarget\wayland-nano` | abort or correct environment |
| Git | provenance/diff/gates only | yes in repository | existing | WP-2 runtime must not invoke Git |

[VERIFIED: environment assignment; repo/toolchain configuration]

No external service or network is required for WP-2 tests; generation uses stubs. [VERIFIED: WP12 §§3.4, 5.4]

## Provenance and Promotion Evidence

Add exactly the authoritative `climb.rs` and `engine.rs` rows from WP12 §5.3 and amend the existing `gate.rs` row for sealed artifacts, OS-temp factory, evidence APIs, exact-ms deadlines/timeout behavior, inventory coherence, and compatibility deviations. Do not add rows for parser/manifest separately because they are contract-defined contents of `engine.rs`. [VERIFIED: GOALS WP-2; WP12 §§5.3, 8]

The builder branch does not merge or push. Before handoff it must prove the exact ownership set, no nested `.git`, no generated-table delta, no WP-3+ surface, exact tests 30–33, full crate, cargo-deny/lock review, full `just gate-all`, bounded Critical/High audit/fix evidence, and canary-clean captures. Integration remains no-ff through the detached integrator followed by another full gate and exact-SHA six-leg CI. [VERIFIED: AGENTS; V3 standing rules; ROADMAP Phase 5 promotion gate]

## Open Questions

None requiring user choice. The owner reconciliation resolved the prior Effects seam, artifact handoff, parser/manifest ownership, runner compatibility, deadline/cancellation, model-pool, and WP-2/WP-3 boundary ambiguities. Implementation may still surface platform-specific filesystem API details; those are constrained engineering work, not authority changes, and any required dependency/feature expansion needs an explicit deviation. [VERIFIED: DEV-WP-2A; frozen IFACE/WP12]

## Assumptions Log

| # | Claim | Risk if wrong |
|---|---|---|
| — | None. This research relies on the hash-frozen project authorities and inspected landed code. | — |

## Sources

### Primary (HIGH confidence)

- `docs/FOLLOWUPS.md` DEV-WP-2A — authority hashes, WP-1 promotion precondition, resolved ownership, and explicit expansion exclusion. [VERIFIED: codebase]
- `F:/Development/waylandnano/shared/reviews/research-0.2/specs/SPEC-WP-INTERFACES.md` §§4, 5, 5A, 8 — canonical public APIs and trust boundaries. [VERIFIED: hash-frozen authority]
- `F:/Development/waylandnano/shared/reviews/research-0.2/specs/SPEC-WP12-nano-verify-engine.md` §§3–8 — climb behavior, implementation ownership, tests, provenance, and definition of done. [VERIFIED: hash-frozen authority]
- `F:/Development/waylandnano/shared/reviews/research-0.2/GOALS.md` WP-2 — objective and scope. [VERIFIED: hash-frozen authority]
- `.planning/REQUIREMENTS.md`, `.planning/ROADMAP.md` — phase requirements and promotion gate. [VERIFIED: codebase]
- `AGENTS.md`, `crates/nano-verify/**`, root manifests/toolchain — current constraints and landed WP-1 API/dependencies. [VERIFIED: codebase inspection]

No external web/package claims were needed; this is a codebase-only, single-technology phase governed by locked internal contracts. [VERIFIED: research scope]

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — inspected landed manifests/toolchain; no new package required.
- Architecture: HIGH — exact hash-frozen IFACE/WP12 contracts.
- Pitfalls: HIGH — derived directly from mandatory test and compatibility clauses.
- Security: HIGH — frozen threat-boundary semantics plus repository fail-closed rules.

**Research date:** 2026-08-20  
**Valid until:** authority bytes or owner reconciliation changes; otherwise stable for this phase.
