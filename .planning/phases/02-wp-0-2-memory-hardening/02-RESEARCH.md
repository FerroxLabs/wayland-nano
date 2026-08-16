# Phase 2: WP-0.2 Memory Hardening - Research

**Researched:** 2026-08-16
**Domain:** Rust ACP-host retained-memory instrumentation, measurement-led correction, Windows PWS soak verification
**Confidence:** HIGH
**Verified baseline:** `566e3acdd6f6fb3025432bf4f8a22e2dec021efe` (`HEAD == origin/master`, clean `feat/wp-0.2` worktree at research time) [VERIFIED: `git rev-parse HEAD origin/master`, 2026-08-16]

## Summary

WP-0.2 is a measurement-first diagnostic phase with two possible terminal shapes. First add an off-by-default `mem-stats` reporter, run the 900-second short profile, and correlate exact retained-structure deltas with Windows private working set. Only if fold auxiliaries or tool-definition clones dominate may the corresponding fix arm land. If neither dominates, stop after recording the profile and leave F-45 open; a speculative fix violates the binding spec. [VERIFIED: `shared/reviews/research-0.2/specs/SPEC-WP0-hardening.md:191-299`; `shared/reviews/research-0.2/GOALS.md:47-63`]

Current source still matches the intended seams: the ACP host owns one `Session`, whose `ContextFold` retains `messages`, `assistant`, `call_names`, `seen`, `covered`, image-manifest ids, and todos; turn completion advances that fold and rerenders the prefix. `TurnEngine` owns a tool-definition vector and clones/remerges it for each model request. [VERIFIED: `crates/nano-cli/src/acp_mode.rs:133-255,1364,2823-2827,4843-4873,6479-6529`; `crates/nano-agent/src/turn.rs:280-296,601-622,1165-1169` at `566e3ac`]

Two authority gaps must be resolved before implementation. The binding spec requires a `nano-cli` package feature, but `crates/nano-cli/Cargo.toml` has no `[features]` table and WP-0.2 ownership grants only root `Cargo.toml` for feature wiring; a virtual workspace root cannot declare a member package feature. Also, the existing fake-model `--mode receipt` run always emits B11 `FAIL`, so the one-hour WP gate can truthfully require B1 PASS but cannot claim the complete manifest is green without separately authorized harness work. [VERIFIED: `crates/nano-cli/Cargo.toml:1-71`; root `Cargo.toml:1-23`; `scripts/soak/soak.mjs:190-203`; `SPEC-WP0-hardening.md:160-163,280-299`]

**Primary recommendation:** Plan three serial waves: contract/deviation closure plus reporter tests; 900-second Windows profile and decision checkpoint; then exactly one selected fix arm plus equivalence/bound tests and the one-hour B1 receipt, or the explicit no-fix stop arm. [VERIFIED: binding WP-0.2 decision procedure]

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|---|---|---|---|
| Retained-structure accounting | ACP host (`nano-cli`) | `nano-agent` only if tool-clone arm selected | Session/fold state and emission cadence live in `acp_mode.rs`; engine clone behavior lives in `turn.rs`. [VERIFIED: current symbols above] |
| PWS measurement | ACP host on Windows | Soak oracle | The record requires process-local PWS, while the existing external oracle uses PowerShell `PrivateMemorySize64`. [VERIFIED: spec schema; `scripts/soak/sample-oracles.ps1:17-22`] |
| Dominance decision | Evidence analysis | — | Per-structure delta must be compared with PWS delta per completed turn before choosing a fix. [VERIFIED: spec lines 234-277] |
| Acceptance | Soak harness/evidence | F-45 ledger | B1 is evaluated from PWS samples; the culprit, fix SHA, and slopes belong in F-45. [VERIFIED: `budget-eval.mjs:50-70`; spec lines 280-299] |

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|---|---|---|
| MEM-01 | Feature-gated exact NDJSON reporter without ACP stdout/stderr contamination | Reporter placement, exact schema, cadence, file-only transport, feature and manifest gap are pinned below. |
| MEM-02 | Short soak identifies fold auxiliaries, tool clones, or neither | Profile command, correlation method, and evidence outputs are pinned below. |
| MEM-03 | Implement only selected arm; stop on neither | A mandatory decision checkpoint separates instrumentation from product fix. |
| MEM-04 | Preserve fold equivalence and assert bounds | Existing oracle and exact compaction fold seam are pinned below. |
| MEM-05 | One-hour receipt meets B1 slope ≤16 MiB/hour | All three locked B1 checks, command, Windows PWS source, and B11 caveat are pinned below. |
</phase_requirements>

## Project Constraints (from AGENTS.md)

- Write only inside the assigned repository/shared scope; Track A and donor snapshots are read-only. [VERIFIED: `AGENTS.md:30-43`]
- Every changed line must trace to WP-0.2, and file ownership is absolute; a boundary conflict stops for a deviation request. [VERIFIED: `AGENTS.md:7-28`; master plan §Standing execution rules]
- Never read, echo, copy, or embed the Flux key; reference `../.secrets/flux-test-key` by path only and canary-scan every capture. [VERIFIED: `AGENTS.md:47-62`; master plan rule 5]
- Keep fail-closed sandbox, egress, policy, and journal invariants; do not weaken code or tests to pass. [VERIFIED: `AGENTS.md:64-75`]
- Use `NANO_*` names for Wayland Nano environment variables and preserve ACP stdout as the wire. [VERIFIED: `AGENTS.md:77-83`; binding spec lines 209-232]
- Rust is pinned by `rust-toolchain.toml` and the required completion gate is `just gate-all` (fmt, clippy `-D warnings`, workspace tests, generator checks). The installed active tools report Rust/Cargo 1.95.0. [VERIFIED: `AGENTS.md:84-94`; `justfile:8-37`; local `rustc --version`, `cargo --version`]
- No commit or push is authorized for this research artifact. [VERIFIED: `AGENTS.md:118-122`; parent assignment]

## Exact Ownership and Authority

Permitted product/evidence surfaces are `crates/nano-cli/src/acp_mode.rs`, `crates/nano-agent/**`, root `Cargo.toml` only for feature wiring, `scripts/soak/evidence/**`, and `docs/FOLLOWUPS.md`. Budgets are owner-locked. [VERIFIED: `GOALS.md:61-63`; `SPEC-WP0-hardening.md:160-163`]

Planning consequences:

1. Do not edit `scripts/soak/soak.mjs`, `sample-oracles.ps1`, `budget-eval.mjs`, `budgets.json`, `justfile`, `crates/nano-protocol/**`, or generated artifacts under the current grant. [VERIFIED: exclusion by exact OWNS list]
2. Reporter implementation and its in-module unit tests fit `acp_mode.rs`. A tool-clone fix and tests fit `crates/nano-agent/**`. Evidence run directories and F-45 updates fit the grant. [VERIFIED: exact OWNS list]
3. Before coding, obtain an explicit one-file deviation granting `crates/nano-cli/Cargo.toml`; this is the only Cargo manifest that can define the required `nano-cli` `mem-stats` feature. Root `Cargo.toml` is a virtual workspace manifest and has no package feature table. [VERIFIED: current Cargo manifests; Cargo feature ownership follows the package manifest]
4. If process-local Windows PWS uses `windows-sys::Win32::System::ProcessStatus::GetProcessMemoryInfo`, the same deviation must permit adding the required API feature to the existing `windows-sys = 0.52` declaration in `crates/nano-cli/Cargo.toml`; no new crate is needed. If owner will not grant it, the reporter cannot satisfy the exact `pws_bytes` record from owned code and must stop. [VERIFIED: existing `windows-sys` declaration at `crates/nano-cli/Cargo.toml:70-71`; exact schema requirement]

## Standard Stack

### Core

| Component | Version | Purpose | Direction |
|---|---:|---|---|
| Rust / Cargo | 1.95.0 active | Compile feature-gated host instrumentation and tests | Use the pinned project toolchain; add no external package. [VERIFIED: local tool versions and `rust-toolchain.toml`] |
| `serde` / `serde_json` | existing workspace resolution | Exact record serialization | Reuse existing dependencies; `MemStatsRecord` derives `Serialize` and `Deserialize` with `deny_unknown_fields`. [VERIFIED: spec schema; `crates/nano-cli/Cargo.toml:46-47`] |
| `windows-sys` | 0.52 existing | Process-local Windows private-memory query | Reuse existing pin with the narrowly required API feature, subject to ownership deviation. [VERIFIED: `crates/nano-cli/Cargo.toml:68-71`; AGENTS pin] |
| Node soak harness | Node 24.16.0 installed | 900-second profile and 3600-second receipt | Use current `scripts/soak/soak.mjs`; do not modify locked harness. [VERIFIED: local `node --version`; exact OWNS list] |
| Windows PowerShell | 5.1.26100.8875 installed | Existing external PWS oracle | Preserve `PrivateMemorySize64` sampling as independent comparison. [VERIFIED: local probe; `sample-oracles.ps1:19`] |

No package installation is required; therefore no Package Legitimacy Audit applies. [VERIFIED: spec says no new deps]

## Architecture Patterns

### System Architecture Diagram

```text
ACP session/prompt
  -> Session.turn_counter increments
  -> TurnEngine builds/runs request
  -> journal TurnEnd
  -> ContextFold.advance
  -> if feature mem-stats AND NANO_MEM_STATS is a valid path
       -> every 25 completed turns
       -> approximate retained bytes + process PWS
       -> one serialized NDJSON record appended to the file
  -> ACP response remains stdout-only; diagnostics remain separate

900-second soak -> mem-stats.ndjson + soak-samples.ndjson
  -> compare structure delta/turn with PWS delta/turn
  -> fold auxiliaries | tool clones | neither
  -> selected fix only | selected fix only | stop and file evidence
```

### Recommended Project Structure

```text
crates/nano-cli/src/acp_mode.rs       # reporter, size accounting, cadence, tests
crates/nano-cli/Cargo.toml            # deviation-required feature/API wiring
crates/nano-agent/**                   # only if tool-definition arm wins
scripts/soak/evidence/run-<stamp>/     # ignored raw run output; explicitly stage approved evidence if required
docs/FOLLOWUPS.md                      # F-45 measured result/status
```

The evidence directory ignores every run except `.gitignore`; planning must include an explicit evidence-retention decision because raw runs will not be tracked automatically. [VERIFIED: `scripts/soak/evidence/.gitignore:1-2`; `git ls-tree`]

### Pattern 1: Feature-compiled, runtime-opt-in reporter

Compile all reporter code behind `#[cfg(feature = "mem-stats")]`; with the feature absent, there must be no environment read, file creation, PWS query, or output. With the feature present but `NANO_MEM_STATS` absent, behavior remains inert. [VERIFIED: binding requirement “off by default”; MEM-01]

Resolve the path once at ACP-host startup, never once per record. Open in append/create mode and keep a host-owned writer. A write/open/serialization error must not be written to ACP stdout; a bounded stderr diagnostic is acceptable only for reporter failure, while stats records themselves are file-only. [ASSUMED: implementation pattern consistent with the locked transport; exact failure policy is not specified]

### Pattern 2: Measure the post-turn retained state

Emit after `fold.advance`, prefix refresh, and override clearing at the completed-turn boundary, not before the turn. The existing monotonic `Session.turn_counter` is restored from journal `TurnBegin` count and increments at prompt start; cadence should use completed successful fold boundaries so the record’s `turn` meaning remains honest across load/resume. [VERIFIED: `acp_mode.rs:1875-1879,2823-2827,4843-4873`; record comment “turns completed”]

### Pattern 3: Approximate allocation accounting with explicit formulas

For `Vec<T>`, account `capacity * size_of::<T>()` plus heap-owned contents; for strings, add capacities; for maps/sets, use a consistent documented approximation including key/value heap contents. Do not present these as allocator-exact heap bytes. The phase needs relative deltas sufficient to name the dominant structure. [VERIFIED: spec lines 196-205]

Required serialized field names, with no extras:

```rust
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MemStatsRecord {
    ts: String,
    pid: u32,
    turn: u64,
    fold_messages: u64,
    fold_assistant: u64,
    fold_call_names: u64,
    fold_seen: u64,
    fold_covered: u64,
    fold_uncompacted_image_manifests: u64,
    fold_todos: u64,
    prefix_cache: u64,
    context_override: u64,
    sessions_map: u64,
    mcp_registry: u64,
    pws_bytes: u64,
}
```

[VERIFIED: `SPEC-WP0-hardening.md:215-232`]

At current `566e3ac`, the ACP host has `let mut session: Option<Session>`, not a sessions map. Therefore `sessions_map` should account the retained optional session/container itself and its owned session fields once, not invent a nonexistent map. State that compatibility interpretation in code/tests. [VERIFIED: `acp_mode.rs:1364`; negative claim checked by repository grep]

### Pattern 4: Profile-selected branching

- Fold arm: only if `fold_seen`, `fold_covered`, and/or `fold_call_names` explain the dominant delta, prune dead covered identifiers at `CompactionComplete`, then extend equivalence tests with size bounds. [VERIFIED: spec lines 263-269; current compaction apply at `acp_mode.rs:6887-6910`]
- Tool arm: only if tool-definition clone accounting explains the dominant delta, cache base or merged definitions keyed to MCP registry generation without breaking mid-turn hydration. [VERIFIED: spec lines 270-273; current `ToolExecutor::current_mcp_tool_definitions` contract at `turn.rs:153-161`]
- Neither arm: update F-45 with all measured deltas and leave it open; land no speculative correction. [VERIFIED: spec lines 274-278,292-299]

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---|---|---|---|
| Heap profiler first | New profiler integration | Exact retained-structure counters + existing PWS oracle | This is the prescribed lightest diagnostic; cdb/ETW is fallback only if inconclusive. [VERIFIED: spec lines 191-239] |
| Timestamp dependency | Add a date/time crate | Existing project timestamp approach or a small owned formatter | No new dependency is allowed. The exact ISO-8601 UTC format still needs a unit test. [VERIFIED: spec; project dependency constraint] |
| Alternate soak evaluator | New budget math | Existing `evaluateB1` | It enforces absolute, end ratio, and slope together. [VERIFIED: `budget-eval.mjs:46-70`] |
| Secret scanning | Copy or print the key | Existing `scripts/canary/scan.mjs` operated by authorized owner/integrator | The scanner holds only an in-memory key and emits a fingerprint receipt. [VERIFIED: `scripts/canary/scan.mjs:1-25,64-95`] |

## Common Pitfalls

### Pitfall 1: Defining the feature in the wrong manifest
**What goes wrong:** Root `Cargo.toml` cannot create `nano-cli/mem-stats`; `unexpected_cfgs` then fails clippy if the package feature is undeclared. [VERIFIED: current virtual workspace/package manifests]
**Avoid:** obtain the manifest deviation before implementation and add `[features] default = []; mem-stats = []` to `crates/nano-cli/Cargo.toml`. [ASSUMED: exact minimal feature syntax]

### Pitfall 2: Polluting ACP channels
**What goes wrong:** stdout corrupts JSON-RPC; record lines on stderr can interleave with receipts/warnings. [VERIFIED: spec lines 209-214]
**Avoid:** stats go only to the configured append file; add an integration-style test that captures stdout/stderr and parses the stats file separately.

### Pitfall 3: Counting transient clones as retained memory
**What goes wrong:** per-request tool vectors or prompt materializations may be large but drop at turn end. [VERIFIED: `turn.rs:1165-1169`; spec lines 178-187]
**Avoid:** account at the post-turn retained-state boundary and choose a fix only from monotonic retained deltas correlated with PWS.

### Pitfall 4: Pruning `call_names` too aggressively
**What goes wrong:** tool result pairing can break if a covered call id is removed while a surviving result still needs it. [VERIFIED: `ContextFold.call_names` semantics at `acp_mode.rs:6487-6493`]
**Avoid:** derive the exact prune set from compaction coverage and preserve the full-rebuild equivalence oracle at every tested boundary.

### Pitfall 5: Treating a 15-minute slope as acceptance
**What goes wrong:** the short run selects the arm; it does not close F-45. [VERIFIED: GOALS completion criterion and spec acceptance]
**Avoid:** reserve the one-hour run until after the selected fix and unit gates; enforce the one-hour budget literally.

### Pitfall 6: Calling the whole receipt manifest green
**What goes wrong:** current fake-only receipt mode hard-codes B11 FAIL. [VERIFIED: `soak.mjs:202`]
**Avoid:** record and gate the B1 result explicitly for WP-0.2; describe B11 as an existing unrelated live-segment failure unless ownership is expanded.

### Pitfall 7: Losing evidence because it is ignored
**What goes wrong:** `scripts/soak/evidence/run-*` is ignored by Git. [VERIFIED: evidence `.gitignore`]
**Avoid:** planner must specify whether the owner will force-add canary-clean run artifacts or retain them externally and commit only a digest/summary under an authorized path.

## Validation Architecture

### Test Framework

| Property | Value |
|---|---|
| Framework | Rust built-in tests plus dependency-free Node budget test |
| Config | workspace Cargo manifests; `justfile` |
| Quick run | `cargo test -p nano-cli acp_mode::tests::mem_stats --features mem-stats` (test name to create) |
| Existing fold oracle | `cargo test -p nano-cli incremental_fold_matches_full_rebuild` |
| Budget oracle | `node scripts/soak/test-budgets.mjs` |
| Full phase gate | `just gate-all` plus feature build/test, short profile, selected-arm tests, and one-hour B1 run |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command / Evidence | Exists? |
|---|---|---|---|---|
| MEM-01 | exact deny-unknown schema; 25-turn cadence; inert without env/feature; append-only file; no ACP contamination | unit + host integration | focused `nano-cli` tests with temp path and captured channels | ❌ Wave 0 |
| MEM-02 | 900-second measured correlation | Windows evidence | `NANO_MEM_STATS=<run path> node scripts/soak/soak.mjs --mode ci --duration-seconds 900 --binary <feature build>` | harness ✅; reporter/evidence ❌ |
| MEM-03 | only measured arm runs | human decision checkpoint + diff audit | profile table naming fold/tool/neither | ❌ Wave 1 |
| MEM-04 | semantic equality and bounded auxiliaries/cache | regression | existing `incremental_fold_matches_full_rebuild*` plus new size assertion | equivalence ✅; bound ❌ |
| MEM-05 | B1 absolute/end-ratio/slope pass for 3600 seconds | Windows acceptance | `node scripts/soak/soak.mjs --mode receipt --duration-seconds 3600 --binary <fixed feature build>` and inspect manifest B1 | harness ✅; receipt ❌ |

### Required Wave 0 Tests

- Exact serialization field set and `deny_unknown_fields` rejection.
- Disabled/default build has no reporter side effects; enabled/no-env remains inert.
- Enabled/path emits one line at turn 25, appends at turn 50, and every line parses independently.
- Unwritable path fails without stdout corruption or panic; lock the expected diagnostic behavior.
- PWS query returns a nonzero value on Windows; unsupported-platform behavior is explicit and cannot masquerade as acceptance.
- Size-accounting unit fixtures prove monotonic growth for each retained collection and stable accounting for empty/default state.
- Build command must combine both features: `cargo build --release -p nano-cli -F nano-agent/soak-fake-model -F nano-cli/mem-stats`. [VERIFIED: existing fake-model preflight at `soak.mjs:29-40`; required new feature]

### Sampling and One-Hour Budget

The reporter emits every 25 completed turns, while the external soak PWS oracle samples once per minute outside smoke mode. The one-hour B1 result passes only if peak PWS ≤1,610,612,736 bytes, final/baseline-median ratio ≤1.25, and fitted slope ≤16,777,216 bytes/hour. [VERIFIED: `soak.mjs:147-154`; `budgets.json:2`; `budget-eval.mjs:50-70`]

The harness spreads the caller environment into each restarted host, so setting `NANO_MEM_STATS` on the Node invocation reaches every PID and all restarts append to the same path. Analysis must segment PWS by PID or explicitly account for restart discontinuities before correlating slopes. [VERIFIED: `soak.mjs:90-94,139-169`; second sentence is required inference]

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---|---|---|
| V2 Authentication | no | No authentication surface changes. [VERIFIED: scope] |
| V3 Session Management | yes | Reporter observes but must not change session lifecycle or journal replay. [VERIFIED: reporter placement] |
| V4 Access Control | yes | The user-supplied stats path is an opt-in diagnostic sink; do not broaden tool or sandbox authority. [ASSUMED: path threat classification] |
| V5 Input Validation | yes | Validate/nonempty path handling, fail boundedly on open/write errors, serialize records rather than interpolate JSON. [ASSUMED: secure implementation pattern] |
| V6 Cryptography | no | No cryptographic function is introduced. [VERIFIED: scope] |
| V7 Error Handling / Logging | yes | Never place stats or secret-bearing environment data on ACP stdout/stderr. [VERIFIED: binding transport and AGENTS secrets rules] |

### Threats and Controls

| Pattern | STRIDE | Control |
|---|---|---|
| Stats path points to sensitive/privileged target | Tampering | Use normal OS append permissions, no privilege elevation, no fallback path, and fail closed for the reporter. [ASSUMED] |
| Record includes conversation/tool content | Information disclosure | Emit numeric sizes only; never strings, environment values, model content, paths, or key material. [VERIFIED: exact closed schema] |
| Concurrent/interleaved records | Tampering | One host-owned writer per process and one complete serialized line per append; restarts append, never truncate. [ASSUMED] |
| Evidence captures credential | Information disclosure | Use fake placeholder workload, do not read the real key during soak, then run the authorized canary scan over captured evidence before promotion. [VERIFIED: `soak.mjs:93`; AGENTS/master rules] |

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|---|---|---:|---|---|
| Rust/Cargo | build/tests | ✓ | 1.95.0 | — |
| Node | soak | ✓ | 24.16.0 | — |
| Windows PowerShell | PWS oracle | ✓ | 5.1.26100.8875 | — |
| WPR/WPA | fallback ETW | ✓ | Windows kit installed | cdb if separately installed |
| cdb | fallback heap snapshots | ✗ in PATH | — | ETW via WPR/WPA |

[VERIFIED: local command probes on 2026-08-16]

## Open Questions / Mandatory Checkpoints

1. **Manifest ownership deviation (blocking before code):** Grant `crates/nano-cli/Cargo.toml` for `[features] mem-stats` and, if chosen, the existing `windows-sys` ProcessStatus feature. Without this, MEM-01 is not implementable within ownership. [HIGH]
2. **`sessions_map` compatibility meaning:** Approve accounting the current `Option<Session>` host container under the locked field name rather than inventing a map. [HIGH]
3. **Reporter failure behavior:** The spec fixes transport but not whether an unwritable `NANO_MEM_STATS` path aborts host startup or disables diagnostics with a warning. Recommendation: fail host startup when explicitly configured, because silent loss makes the profile unverifiable. [ASSUMED; user/owner decision]
4. **Evidence retention:** Raw run directories are ignored. Decide force-add versus external retention plus committed digest/summary before spending the hour. [HIGH]
5. **Receipt semantics:** Confirm WP-0.2 acceptance is B1-specific despite the harness’s unrelated hard-coded B11 failure. Do not rewrite the harness without expanded ownership. [HIGH]
6. **Dominance threshold:** The spec says “dominant” and “≈” but gives no numeric threshold. Recommendation: pre-register a rule before profiling: a suspect arm wins only if its combined delta explains a clear majority of positive retained-byte delta and tracks PWS direction across intervals; otherwise choose neither. The exact threshold needs owner approval to prevent post-hoc selection. [ASSUMED]

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|---|---|---|
| A1 | Keep one append writer; warn or fail boundedly on I/O errors | Architecture Pattern 1 | Protocol or availability behavior could differ from owner intent. |
| A2 | Minimal feature syntax is `[features] default=[]; mem-stats=[]` | Pitfall 1 | Feature composition may require additional forwarding. |
| A3 | Stats path should fail startup when explicitly invalid | Open Questions | A warning-only policy may be desired. |
| A4 | Pre-register a quantitative dominance rule | Open Questions | Owner may supply a different analysis rule. |
| A5 | Numeric-only records and no elevation are sufficient path controls | Security | Owner may require repo confinement or secure-create semantics. |

## Sources

### Primary (HIGH confidence)

- `shared/reviews/research-0.2/NANO-BUILD-PLAN-V3.md` — binding frame, order, ownership discipline, evidence and secrets rules.
- `shared/reviews/research-0.2/specs/SPEC-WP-INTERFACES.md` — read completely; WP-0.2 does not touch its verifier interfaces.
- `shared/reviews/research-0.2/GOALS.md:47-63` — objective, terminal stop arm, completion, ownership.
- `shared/reviews/research-0.2/specs/SPEC-WP0-hardening.md:155-299` — exact reporter, schema, decision arms, acceptance.
- `AGENTS.md`; `.planning/ROADMAP.md`; `.planning/REQUIREMENTS.md`; `.planning/STATE.md`; `.planning/config.json` — repository and phase controls.
- Current code/harness at commit `566e3ac`: `acp_mode.rs`, `turn.rs`, Cargo manifests, `scripts/soak/**`, `docs/FOLLOWUPS.md`, `justfile`, canary scanner.

No web research was needed: this is a codebase-only, binding-spec phase with no new package selection. [VERIFIED: research scope]

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — installed versions and existing manifests directly inspected.
- Architecture: HIGH — current symbols and call sites inspected at exact baseline.
- Pitfalls: HIGH for contract/code mismatches; MEDIUM for recommended failure/dominance policies awaiting owner decision.
- Security: HIGH for locked protocol/secret constraints; MEDIUM for path-policy recommendation.

**Research date:** 2026-08-16
**Valid until:** current `origin/master` changes any cited ACP/fold/soak surface; otherwise 30 days.
