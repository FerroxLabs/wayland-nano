# Phase 6: WP-3 Verify CLI and CI Surface - Research

**Researched:** 2026-08-21
**Domain:** Rust CLI orchestration, trusted Git patch materialization, offline receipt verification, pinned CI consumption
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- D-01: Implement only WP-3: CLI mint, run-only, offline receipt verification, JSONL v1,
  the empty schema-1 registry bootstrap, owned fixtures/docs/CI consumer, and provenance.
- D-02: Preserve the exact ownership fences in the WP-3 spec. In particular, WP-3 does not
  edit `crates/nano-verify/**`, populate production Gate Cards, or promote `.github/**`.
- D-03: Use the exact CLI modes, exit codes, trust boundaries, detached-worktree verification,
  materializer rules, model/deadline inputs, event vocabulary, and 13 named tests from the
  authoritative WP-3 spec.
- D-04: Start from exact green master `d7f4d3a2260f6d08e026fcb1263448355a7f175b` in the
  F-only worktree `.tmp-wt-vc-wp-3` on `feat/wp-3`; builder does not merge or push.
- D-05: Promotion remains one Critical/High audit, at most one consolidated fix round,
  full local gate, detached no-ff integration, exact-SHA six-leg CI, then WP-4. No WP-5,
  WP-6, DeepSeek, profile, memory, MCP, or external-agent expansion is authorized.

### the agent's Discretion

No discretion section exists in `06-CONTEXT.md`.

### Deferred Ideas (OUT OF SCOPE)

No deferred-ideas section exists in `06-CONTEXT.md`; D-05 expressly excludes WP-5, WP-6, DeepSeek, profile, memory, MCP, and external-agent expansion.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CLI-01 | Exact mint, run-only, and receipt-check argv/exit contract with confined run-only artifacts | Parser/mode architecture, exit matrix, registry/materialization guidance, tests 9 and 13 [VERIFIED: local REQUIREMENTS.md and authoritative WP3 spec] |
| CLI-02 | Registry-backed production adapter, climb, and verified-closure-only minting | Responsibility map, imported API inventory, baseline/climb/materializer pipeline, tests 1/2/10 [VERIFIED: local source and authoritative WP3 spec] |
| CLI-03 | Closed JSONL v1 vocabulary without trust-boundary leakage | Event-sink pattern and output rules, tests 1/2 [VERIFIED: local exec_mode pattern and authoritative WP3 spec] |
| CLI-04 | Detached fix-commit worktree, bounded rerun, cleanup, fail-closed verdict | Offline verifier pipeline and tests 3-8/11/12 [VERIFIED: local receipt API and authoritative WP3 spec] |
| CLI-05 | Complete authored-defect, repair, roundtrip, tamper, and exit battery | Exact 13-test validation map below [VERIFIED: authoritative WP3 spec §8] |
| CLI-06 | Schema-pinned CI consumer under docs; no WP-3 `.github` promotion | CI ownership/deletion-hole guidance and post-WP-4 deferral [VERIFIED: authoritative WP3 spec §7] |
| PROV-02 | Exact donor transformation provenance | Owned `UPSTREAM.md` row task [VERIFIED: local REQUIREMENTS.md and AGENTS.md] |
</phase_requirements>

## Summary

WP-3 is an orchestration and trust-boundary phase, not a verifier-engine phase. The landed `nano_verify` crate at base `d7f4d3a2260f6d08e026fcb1263448355a7f175b` already exports the sealed candidate workspace/artifact, baseline execution, climb, canonical registry/digest, receipt preflight/store, patch parser, and expected-change manifest required by the contract. WP-3 must import those types and keep all new CLI behavior in the narrowly owned `verify_cmd.rs`, with only the specified registration/dependency/module edits elsewhere. [VERIFIED: `nano-verify/src/lib.rs` and authoritative WP3 spec §§0-1]

The hard part is the minting transaction: prove a clean F:-resident source tree and matching canonical F: `TEMP`/`TMP`; run the red baseline only in a detached start-commit worktree; run the climb through a production `Effects` adapter; consume the sealed accepted bytes; validate and apply exactly one Git patch using nano-verify's parser and manifest as the sole oracle; create one coherent fix commit; rerun the pinned gate; then and only then mint/store/copy a receipt. Every scheduled effect shares one absolute monotonic deadline. [VERIFIED: authoritative WP3 spec §§2,5]

Offline checking is a separate, model-free transaction: WP12 preflight first, then a bounded detached fix-commit worktree rerun owned by WP-3, unconditional cleanup, and a closed verdict. CI consumes this command from documentation-owned pinned workflows; `.github/**` remains outside builder ownership. [VERIFIED: authoritative WP3 spec §§6-7]

**Primary recommendation:** Plan WP-3 as six dependency-ordered implementation slices—parser/events, Git/registry/deadline primitives, offline verifier, production Effects + mint orchestration, deterministic materializer, fixtures/docs/provenance—then run the exact 13 named tests and full gates. [VERIFIED: authoritative WP3 spec §§8-10]

## Project Constraints (from AGENTS.md)

- Work only in the assigned F: worktree and owned WP-3 paths; Track A/upstreams are read-only, D: is rollback-only, and shared files are not owned in this phase. [VERIFIED: AGENTS.md; 06-CONTEXT.md]
- Do not read, print, copy, or embed secret values; verification tests are offline and must not depend on a Flux key. [VERIFIED: AGENTS.md; authoritative WP3 spec §8]
- Fail closed; do not weaken sandbox, egress, gate, receipt, or tests to get green. [VERIFIED: AGENTS.md]
- Rust 1.95.0, edition 2024, native MSVC, `windows-sys` 0.52; clippy warnings are errors. [VERIFIED: AGENTS.md; local `rustc --version`]
- Run `just gate-all` before completion; focused tests do not replace the full gate. [VERIFIED: AGENTS.md]
- Every donor-adapted file needs an exact `UPSTREAM.md` transformation row. [VERIFIED: AGENTS.md]
- Builder does not commit, merge, push, self-approve, edit owner-managed status, or promote `.github/**`. [VERIFIED: AGENTS.md; 06-CONTEXT.md]

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Arg parsing/mode validation/exit mapping | CLI boundary (`verify_cmd.rs`) | thin `main.rs` dispatch | WP-3 owns the exact user contract; main remains registration-only. [VERIFIED: WP3 §§2-3] |
| Registry, canonical digest, inventories | `nano_verify` imported core | CLI resolver | CLI selects entries/materializes cwd and timeout but must not recreate canonical digest or inventory logic. [VERIFIED: local exports; IFACE §§1-4] |
| Candidate generation | CLI production `Effects` adapter | existing provider stack | Adapter owns generation/time/cancel/sanitized events only; core owns candidate and gate I/O. [VERIFIED: `engine::Effects`; WP3 §5.4] |
| Climb/gate execution | `nano_verify` trusted core | CLI orchestration | `run_climb`, sealed workspace/artifact, baseline/gate execution remain engine-owned. [VERIFIED: local exports] |
| Patch application and commit | CLI trusted materializer | Git subprocess | WP-3 owns mutation, while nano-verify's parsed diff and sealed manifest are the sole expected-result oracle. [VERIFIED: WP3 §5 materializer] |
| Receipt preflight/store | `nano_verify` | CLI final rerun/verdict | WP12 proves read-only prerequisites; WP-3 alone creates/removes fix worktree and derives `Valid`. [VERIFIED: `receipt.rs`; WP3 §6] |
| CI consumption | `docs/verify/ci/**` | post-WP-4 owner lane | WP-3 authors pinned docs-only consumers; `.github/workflows/**` promotion waits until after WP-4 sealed mutants land. [VERIFIED: WP3 §§1,7] |

## Standard Stack

### Core

| Library/API | Version | Purpose | Why Standard |
|-------------|---------|---------|--------------|
| `nano-verify` workspace crate | 0.1.0 path dependency | Registry, gate, climb, candidate, receipt, patch/manifest contracts | Mandatory imported authority; no alternative is permitted. [VERIFIED: local Cargo/lib source and WP3 §0] |
| Rust standard library `Command`, filesystem/path/time | Rust 1.95.0 | Git probes/worktrees, path confinement, monotonic control wiring | Existing repository pattern; no new package required. [VERIFIED: local source/toolchain] |
| `serde` / `serde_json` | workspace-resolved existing dependencies | Closed JSONL/verdict serialization | Already present in nano-cli and used by existing event sinks. [VERIFIED: `nano-cli/Cargo.toml`, `exec_mode.rs`] |
| `tokio` | existing 1.x dependency | async climb/gate/provider execution | Existing current-thread CLI runtime and nano-verify async API. [VERIFIED: manifests/main.rs] |
| Git CLI | 2.54.0 locally | authoritative tree/index/worktree/commit probes | Spec names exact Git operations and Git is already an environmental dependency. [VERIFIED: local tool output; WP3 §§5-6] |

### Supporting

| Tool | Version | Purpose | When to Use |
|------|---------|---------|-------------|
| `actionlint` | 1.7.7 locally | Workflow syntax/static validation | Validate both docs-owned workflows before phase gate. [VERIFIED: local tool output; WP3 DoD] |
| `just` | 1.51.0 locally | Canonical repo gates | Focused tasks plus final `just gate-all`. [VERIFIED: local tool output; AGENTS.md] |

**Installation:** No new external package is authorized or needed. Add only the specified workspace path dependency `nano-verify = { version = "0.1.0", path = "../nano-verify" }`. [VERIFIED: WP3 §1]

## Package Legitimacy Audit

Not applicable: WP-3 installs no external package; it adds one in-repository workspace path dependency already landed and audited. [VERIFIED: WP3 ownership contract and manifests]

## Architecture Patterns

### System Architecture Diagram

```text
argv
  -> parse_args (closed Mint | CheckReceipt | RunOnly)
      -> entry preflight (repo/F:/TEMP/TMP/clean/deadline)
          -> registry selection + canonical inventory/closure
              +-> RunOnly -> confined artifact -> bounded gate -> 0/3
              +-> CheckReceipt -> WP12 preflight -> detached fix worktree
              |                  -> bounded pinned rerun -> cleanup -> 0/6
              +-> Mint -> detached baseline-red proof
                         -> sealed artifact workspace + run_climb(Effects)
                         -> accepted bytes -> nano_verify parse + sealed manifest
                         -> confined git apply/check/index verification
                         -> one fix commit -> pinned green rerun
                         -> atomic store + optional atomic copy -> 0
```

### Recommended Project Structure

```text
crates/nano-cli/src/verify_cmd.rs              # all WP-3 CLI/orchestration/materializer logic
crates/nano-cli/tests/verify_cmd.rs            # exact 13 named tests + helpers
crates/nano-cli/tests/fixtures/verify/          # content only; runtime helper creates .git
docs/verify/VERIFY-CLI.md                       # argv, exits, events, receipt honesty
docs/verify/CI-ADOPTION.md                      # post-WP-4 owner adoption/branch-protection procedure
docs/verify/ci/verify-receipt-check.yml         # pinned receipt consumer
docs/verify/ci/verify-dogfood.yml               # future WP-4 dogfood consumer, not promoted
gates/registry.json                             # exact empty schema-1 bootstrap only
UPSTREAM.md                                     # WP-3 transformation entry
```

### Pattern 1: Parse into a closed mode before side effects

`parse_args` must reject illegal combinations, duplicates, missing values, model-ladder violations, and deadline overflow/caps before `run` performs any effect. Preserve `json` and resolved default/explicit deadline in `VerifyMode`; return only `Err(2)` for usage. [VERIFIED: WP3 §2]

### Pattern 2: One dependency-injected orchestration seam

Production `run` should be generic-free, while an internal `run_with` receives scripted clock/generation/gate/Git fault seams used by tests 1, 2, 9, 10, and 13. This matches the landed `run_exec_with`/`run` split and is necessary to prove exact deadline and rollback behavior without live model calls. [VERIFIED: `exec_run.rs:27-52,694`; WP3 §8]

### Pattern 3: Absolute deadline, checked before every scheduled operation

Construct `RunDeadline` once with checked addition. Before every provider, artifact, gate, Git, worktree, and receipt-store action, sample trusted monotonic time. Gate timeouts are `min(pinned_timeout, exact_remaining_ms)`; zero/invalid remainder starts nothing. Receipt checking uses only `NANO_VERIFY_RECEIPT_BUDGET_MS` (120,000 default; 600,000 cap), never the mint/run-only deadline flag. [VERIFIED: WP3 §§2,6]

### Pattern 4: Transactional deterministic materializer

Read accepted bytes once, enforce 16 MiB/UTF-8/raw-diff rules, call `parse_candidate_diff` once and `derive_expected_changes` once, retain the sealed manifest/digests through staged and committed comparisons, and let Git perform `apply --check --index --whitespace=error-all` then the identical apply. Before commit, every failure rolls back and proves exact restoration; after commit, never rewrite history. [VERIFIED: WP3 §5]

### Pattern 5: Cleanup guard for detached worktrees

Treat worktree creation/removal as a resource lifetime. Every success, red result, timeout, panic-converted error, and probe failure must attempt `git worktree remove --force`, then prune/verify absence as planned. A cleanup failure changes the result to fail-closed (`Unverifiable` for receipt checking; runtime/fail-closed class for minting). [VERIFIED: WP3 §§5-6]

### Anti-Patterns to Avoid

- Redeclaring any nano-verify type or implementing a second closure digest, inventory parser, patch parser, hunk applier, operation classifier, or postimage reconstructor. [VERIFIED: WP3 §§0,5,8]
- Running the baseline in the mutable source checkout, deriving red evidence from a candidate/climb outcome, or minting from absent/zero exit or absent digest. [VERIFIED: WP3 §5]
- Treating `log_digest` as independently recomputable proof; only its structure can be checked offline. [VERIFIED: WP3 §6]
- Using shell command strings, accepting provider paths, following symlinks/submodules, normalizing hostile paths, or allowing protected-path overlap. [VERIFIED: IFACE §3; WP3 §5]
- Writing production Gate Cards, editing `nano-verify`, or promoting docs workflows into `.github`. [VERIFIED: WP3 §1]

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Registry/digest/inventory | Local JSON/digest/card logic | `load_registry`, `gate_for_requirement`, `closure_digest`, `check_inventory`, `check_closure_pin` | Prevents canonicalization and pin drift. [VERIFIED: local exports] |
| Candidate identity/storage | Caller temp root/raw path | `create_artifact_workspace()` and `CandidateArtifact::read_exact_bytes()` | Keeps provider and CLI outside artifact identity. [VERIFIED: local gate.rs] |
| Diff semantics | WP-3 parser/reconstructor | `parse_candidate_diff` + `derive_expected_changes` sealed manifest | The manifest is the single expected-result oracle. [VERIFIED: local engine.rs; WP3 §5] |
| Receipt schema/store/preflight | CLI receipt structs/file replacement | imported `Receipt`, `mint_receipt`, `write_receipt`, `preflight_receipt`; existing `atomic_replace_write` for optional copy | Preserves deny-unknown, lock, atomic replace, and locked verification order. [VERIFIED: local receipt.rs; WP3 §§0,6] |
| Gate execution | Direct subprocess parsing | baseline/gate execution APIs | Preserves argv-only spawn, containment, bounded output, full inventory, and fail-closed outcomes. [VERIFIED: local gate.rs] |

## Landed API Compatibility Findings

All contract-critical WP-2 exports are present: `RunDeadline`, `ClimbConfig`, `Effects`, `run_climb`, sealed `ArtifactWorkspace`/`CandidateArtifact`, baseline/gate execution, `CandidateDiff`, `ExpectedChangeManifest`, receipt/preflight/verdict/store, and registry/digest/inventory functions. [VERIFIED: `nano-verify/src/lib.rs` and source signatures]

One planning caveat is real: `load_registry` rejects empty `gates` or `requirements`, while WP-3 must create the byte-exact empty production bootstrap. The plan must explicitly distinguish bootstrap detection from actionable registry loading: recognize the exact bootstrap as a valid no-production-gates state that yields usage exit 2 for requested gates/requirements, while populated fixture/production registries go through nano-verify validation. Do not edit nano-verify and do not generalize this into an alternate registry validator. [VERIFIED: `registry.rs:92-128`; WP3 §§0-1,5]

Production model generation is not supplied by nano-verify and belongs in owned `verify_cmd.rs`. Reuse the existing Flux/provider construction and credential resolution patterns already compiled into nano-cli, but expose only the `Effects::generate` result to nano-verify and never include provider identity/errors in events. Tests 1-2 must use scripted in-process effects and remain keyless/offline. [VERIFIED: `Effects` trait, `exec_run.rs`, WP3 §§5,8]

## Common Pitfalls

### Pitfall 1: Protection checks that are string-prefix based
**What goes wrong:** near-miss names are blocked or actual ancestor/descendant overlaps escape. **How to avoid:** canonical component-aware symmetric overlap; test equality, both ancestry directions, and prefix near misses for every protected item. [VERIFIED: WP3 §5]

### Pitfall 2: Deletion/rename hole in CI
**What goes wrong:** iterating only existing receipt files lets a PR delete proof and pass. **How to avoid:** consume `git diff --name-status`, fail `D*|R*`, verify only `A*|M*`, fetch full history, and map every nonzero verifier exit to job failure. [VERIFIED: WP3 §7]

### Pitfall 3: Long or non-F temporary paths
**What goes wrong:** Windows nested Cargo/Git paths fail or violate the program's disk boundary. **How to avoid:** command-entry canonical matching F: `TEMP`/`TMP` preflight; test fixtures use short F: roots and per-case short target dirs while fixture source remains private. [VERIFIED: WP3 §§5,8; prior repo test pattern]

### Pitfall 4: Cleanup only on the happy path
**What goes wrong:** detached worktrees, patch files, or staged/untracked changes survive timeout/failure. **How to avoid:** resource guards plus injected failure tests at every pre-commit stage; cleanup failure is terminal, never logged as a warning. [VERIFIED: WP3 §§5-6]

### Pitfall 5: Event leakage
**What goes wrong:** free-form `VerifyError`, provider text, paths, argv, diffs, or gate output crosses stdout/stderr. **How to avoid:** closed error codes; `check_verdict` contains only id/category/passed; `climb_update` contains only closed log fields; canary-scan both streams. [VERIFIED: WP3 §§4-5]

### Pitfall 6: Receipt `test` sourced from the wrong field
**What goes wrong:** run artifact, fixture label, task, or provider output is stored. **How to avoid:** copy the selected registry entry's canonical `script` byte-for-byte and verify exact equality offline. [VERIFIED: IFACE §6; WP3 §§0,5-6]

## Exact Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness via Cargo 1.95.0 [VERIFIED: local toolchain] |
| Config file | Workspace `Cargo.toml` / crate `Cargo.toml` [VERIFIED: local tree] |
| Quick run command | `cargo test -p nano-cli --test verify_cmd -- --test-threads=1` |
| Full suite command | `just gate-all` |

### Exact 13 Named Tests

| # | Test | Primary contract coverage |
|---:|------|---------------------------|
| 1 | `verify_full_flow_green_mints_receipt` | Full inventory JSONL, detached red baseline, sealed workspace, materializer teeth, rollback/fix/rerun, byte-identical stores, F-temp discipline [VERIFIED: WP3 §8] |
| 2 | `verify_authored_defect_red_identifiers_only` | Exit 3 and no command/argv/path/expected/provider leakage [VERIFIED: WP3 §8] |
| 3 | `verify_receipt_roundtrip_valid` | Minted receipt re-derived to valid, exit 0 [VERIFIED: WP3 §8] |
| 4 | `verify_receipt_tampered_fails_closed` | Malformed digest gives never-red; explicitly no recomputation claim [VERIFIED: WP3 §8] |
| 5 | `verify_receipt_fabricated_commit` | Absent commit gives fabricated-commit/6 [VERIFIED: WP3 §8] |
| 6 | `verify_receipt_unknown_field_fails_closed` | deny-unknown parse gives unverifiable/6 [VERIFIED: WP3 §8] |
| 7 | `verify_receipt_green_only_is_never_red` | Zero failing exit gives never-red/6 [VERIFIED: WP3 §8] |
| 8 | `verify_receipt_gate_pin_drift` | Closure drift gives gate-mismatch before rerun [VERIFIED: WP3 §8] |
| 9 | `verify_exit_code_matrix` | All invalid combinations, model/deadline bounds, exact deadline narrowing, engine error 1 [VERIFIED: WP3 §8] |
| 10 | `verify_red_run_writes_no_receipt` | Pre-existing output unchanged; no store file [VERIFIED: WP3 §8] |
| 11 | `verify_receipt_ancestry_unproven` | Non-ancestor/missing test path gives ancestry-unproven/6 [VERIFIED: WP3 §8] |
| 12 | `verify_receipt_rerun_red_is_gate_mismatch` | Ready preflight but red fix rerun gives gate-mismatch/6 [VERIFIED: WP3 §8] |
| 13 | `verify_run_only_resolves_artifact_and_exit_codes` | argv shape, confined artifact, 0/3/2 matrix, mutant rejection, exact deadline [VERIFIED: WP3 §8] |

### Requirement-to-Test Map

| Requirement | Automated evidence |
|-------------|--------------------|
| CLI-01 | tests 9, 13; parser unit tables [VERIFIED: WP3 §8] |
| CLI-02 | tests 1, 2, 10 [VERIFIED: WP3 §8] |
| CLI-03 | tests 1, 2 [VERIFIED: WP3 §8] |
| CLI-04 | tests 3-8, 11, 12 [VERIFIED: WP3 §8] |
| CLI-05 | all 13 tests [VERIFIED: WP3 §8] |
| CLI-06 | `actionlint docs/verify/ci/*.yml` plus a deletion/rename shell fixture [VERIFIED: WP3 §§7,10] |
| PROV-02 | ownership diff plus `UPSTREAM.md` row inspection [VERIFIED: AGENTS.md] |

### Sampling Rate

- **Per task:** focused parser/materializer/offline test filter appropriate to the slice. [VERIFIED: repository gate discipline]
- **Per wave:** `cargo test -p nano-cli --test verify_cmd -- --test-threads=1`, strict clippy for nano-cli, and `actionlint` for docs workflows. [VERIFIED: WP3 DoD]
- **Phase gate:** all 13 names observed passing, trust/deletion-hole teeth, `actionlint`, `cargo deny check`, `just gate-all`, ownership diff, canary scan. [VERIFIED: WP3 §§8-10; AGENTS.md]

### Wave 0 Gaps

- [ ] `crates/nano-cli/tests/verify_cmd.rs` and fixture tree do not exist. [VERIFIED: `rg --files`]
- [ ] Exact empty `gates/registry.json` does not yet exist. [VERIFIED: local tree]
- [ ] Docs/workflows under `docs/verify/**` do not yet exist. [VERIFIED: local tree]
- [ ] Scripted `run_with` seams for clock/generation/gate/Git faults must be designed inside `verify_cmd.rs`. [VERIFIED: WP3 §8]

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | Verification flow must be offline in tests and must not invent credentials. [VERIFIED: AGENTS.md; WP3 §8] |
| V3 Session Management | no | Verify uses run ids/events, not user sessions. [VERIFIED: WP3 §4] |
| V4 Access Control | yes | Exact owned-path, registry-target, protected-path, F-drive, and worktree confinement. [VERIFIED: WP3 §§1,5] |
| V5 Input Validation | yes | Closed argv modes, deny-unknown artifacts, canonical repo-relative paths, digest/SHA validation, bounded patch/output/time. [VERIFIED: WP3 §§2,5-6] |
| V6 Cryptography | yes | Use canonical SHA-256 functions already provided; never hand-roll digest proof. [VERIFIED: nano-verify exports] |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Provider-supplied path/command execution | Tampering/Elevation | Candidate bytes only, argv-only gate, sealed workspace, Git parser/apply path [VERIFIED: WP3 §5] |
| Symlink/submodule/path traversal | Tampering | Component validation, canonical ancestors, no links/submodules, strict target descendants [VERIFIED: WP3 §5] |
| Gate/receipt self-attestation | Spoofing | Registry closure pin, commit/ancestry/test probes, detached rerun [VERIFIED: WP3 §6] |
| Evidence deletion in CI | Repudiation | name-status deletion/rename failure [VERIFIED: WP3 §7] |
| Secret/internal leakage | Information disclosure | closed events/errors, null gate stderr, identifiers-only frames, canary scans [VERIFIED: AGENTS.md; WP3 §§4-5] |
| Partial apply/receipt publication | Tampering | staged/commit manifest verification, rollback proof, atomic writers, mint-last [VERIFIED: WP3 §5; nano-verify receipt store] |
| Deadline reset/rounding | Denial of service | one checked absolute deadline and exact remainder narrowing [VERIFIED: WP3 §2] |

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|-------------|-----------|---------|----------|
| Rust | build/tests | yes | 1.95.0 | none |
| Cargo | build/tests | yes | 1.95.0 | none |
| Git | all proof/materialization flows | yes | 2.54.0.windows.1 | none |
| just | canonical gates | yes | 1.51.0 | direct commands are diagnostic only, not phase-gate replacement |
| actionlint | workflow validation | yes | 1.7.7 | equivalent YAML/workflow validator if unavailable on another host |
| Node | repository generated gates | yes | 24.16.0 | pinned CI setup per existing workflow |

No blocking local dependency is missing. [VERIFIED: commands run 2026-08-21]

## State of the Art

| Rejected/older approach | Required approach | Impact |
|-------------------------|-------------------|--------|
| Receipt preflight alone claims valid | WP12 `Ready` plus WP-3 detached green rerun | Independent validation of the claimed fix. [VERIFIED: IFACE v1.1/WP3 §6] |
| Remove+rename/ordinary Windows rename | nano-verify atomic store; existing atomic output-copy primitive | No target-missing window. [VERIFIED: IFACE §9; WP3 §0] |
| Provider returns replacement path/file | Provider yields raw diff bytes into sealed core artifact | Prevents arbitrary filesystem authority. [VERIFIED: IFACE §5; WP3 §5] |
| Per-operation refreshed timeout | Single overall absolute deadline | Prevents budget extension. [VERIFIED: WP3 §2] |
| CI loops existing receipts | diff name-status with deletion/rename failure | Closes evidence deletion hole. [VERIFIED: WP3 §7] |

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | None. All implementation recommendations derive from local source, locked context, or the authoritative WP3/interface specs. | all | none |

## Open Questions (RESOLVED)

1. **RESOLVED — How should the exact empty registry bootstrap bypass `load_registry`'s nonempty check?**
   - What we know: the bootstrap must be byte-exact and empty; `load_registry` deliberately rejects empty registries; nano-verify cannot be edited. [VERIFIED: source/spec]
   - Resolution: 06-01-T2 creates the byte-exact bootstrap and the narrow CLI bootstrap-state recognizer; it accepts only the exact canonical bytes and returns unknown-gate/requirement usage errors, while all nonempty registries use nano-verify loading/validation. [RESOLVED: 06-01-T2]

2. **RESOLVED — Which existing provider constructor is the production `Effects::generate` adapter?**
   - What we know: model ids are caller-supplied, the adapter belongs in `verify_cmd.rs`, and existing nano-cli Flux/provider plumbing is available; no live key may enter fixture tests. [VERIFIED: source/spec]
   - Resolution: in owned `verify_cmd.rs`, call public `ProviderRouter::from_env()` then `resolve_binding(model_id, env_reader, now_unix_secs)` to obtain `ProviderBinding`; the private `acp_mode::runtime_driver` is neither callable nor owned. Build the catalog-derived `EgressPolicy`, dispatch the binding wire through public `FluxCompletionsClient` or `AnthropicMessagesClient` wrapped by `ProviderDriver`, create a non-streaming `ModelRequest` containing one user prompt and no tools, call `ModelDriver::complete`, collect only `TextDelta`, and collapse every provider failure to a fixed sanitized error code. Tests remain injected, keyless, and offline. [RESOLVED: 06-04-T2; verified landed public provider APIs]

## Sources

### Primary (HIGH confidence)

- `F:/Development/waylandnano/shared/reviews/research-0.2/specs/SPEC-WP3-verify-cli-ci.md` — complete WP-3 contract, ownership, flows, tests, CI. [VERIFIED: authoritative project spec]
- `F:/Development/waylandnano/shared/reviews/research-0.2/specs/SPEC-WP-INTERFACES.md` — imported registry/gate/climb/receipt contracts. [VERIFIED: authoritative project spec]
- `.planning/phases/06-wp-3-verify-cli-and-ci-surface/06-CONTEXT.md` — locked phase decisions. [VERIFIED: local context]
- `crates/nano-verify/src/{lib,climb,engine,gate,receipt,registry}.rs` — landed public APIs and behavior. [VERIFIED: local source at exact base]
- `crates/nano-cli/src/{main,lib,exec_mode,exec_run}.rs` and manifests/workflows — integration patterns and environment. [VERIFIED: local source at exact base]

No web or package-registry research was needed: this phase is governed by frozen in-repository/external project authorities and installs no external package. [VERIFIED: phase scope]

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — exact local toolchain, manifests, and imported APIs inspected.
- Architecture: HIGH — locked authoritative specs prescribe ownership and flow.
- Pitfalls: HIGH — each maps to an explicit trust-boundary or named test in the spec.

**Research date:** 2026-08-21
**Valid until:** the WP-3 authority or base SHA changes; otherwise 2026-09-20.
