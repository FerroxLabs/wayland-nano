# Phase 5: WP-2 Gated Climb - Pattern Map

**Mapped:** 2026-08-20  
**Branch base inspected:** `7bcbc12fec0624aacbc3953e4f2c7d1a2c4414e0`  
**Files classified:** 8 implementation/provenance/test surfaces  
**Primary analog families:** 5  
**Authority reconciliation:** `docs/FOLLOWUPS.md` DEV-WP-2A (resolved 2026-08-20)

## Frozen Authority

These external files are the final source of truth. Pattern analogs never override them.

| Authority | Frozen byte count | Frozen SHA-256 |
|---|---:|---|
| `shared/reviews/research-0.2/GOALS.md` | 20,917 | `299234b769ab0cdbd91207ed82f512f28d1cbc962d196317e526205f790d3b62` |
| `shared/reviews/research-0.2/specs/SPEC-WP-INTERFACES.md` | 56,256 | `57c147b970094a83650ea95ca69fb586b63a70e586bdc140bc70f8ab1de48a4d` |
| `shared/reviews/research-0.2/specs/SPEC-WP12-nano-verify-engine.md` | 67,006 | `8ae69bb42f492a26a4f8512775947a91e1c6c3ba541797756d1d53634cc5d491` |

DEV-WP-2A freezes the ownership split: trusted `nano-verify` core owns candidate parsing, opaque workspace/artifact construction, bounded gate execution and evidence, inventory reconstruction, monotonic deadlines, cancellation, and expected-change derivation. WP-2 never applies a diff, mutates Git, captures baseline-red evidence, or mints a receipt. WP-3 alone installs the accepted bytes and invokes `derive_expected_changes` after the climb.

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `crates/nano-verify/src/climb.rs` | pure state reducer / model | deterministic transform | `upstream-ferrox-factory/src/gate-climb.cts` | exact behavior, language port |
| `crates/nano-verify/src/engine.rs` | parser + orchestration service | batch transform, sequential request-response | `upstream-ferrox-factory/src/gate-first-executor.cts` plus frozen IFACE §5 | strong loop/prompt analog; authority adds parser, deadlines, cancellation, events, sealed outcome |
| `crates/nano-verify/src/gate.rs` | trusted execution service | bounded subprocess/file-I/O | existing `nano-verify/src/gate.rs::run_gate`; `nano-protocol/src/acp.rs` path-identity defenses | direct extension of landed code plus security-pattern match |
| `crates/nano-verify/src/error.rs` | error vocabulary | typed error transform | existing `VerifyError` | exact local convention; additive `Artifact` use only as frozen |
| `crates/nano-verify/src/lib.rs` | public API facade | re-export | existing `nano-verify/src/lib.rs` | exact |
| `crates/nano-verify/tests/gate_contract.rs` | process-contract test | subprocess observation | existing file's fixture-binary tests | exact harness family |
| `crates/nano-verify` unit/compile-fail tests | unit and API security tests | deterministic transform + async stub | existing module tests; `nano-agent/review_prompt.rs` leakage canary | role match |
| `UPSTREAM.md` | provenance ledger | append-only documentation | existing WP-1 `nano-verify` rows | exact ledger convention |

Root `Cargo.toml` is a landed WP-1 precondition and is not a Phase-5 edit. No new crate or dependency is indicated. `tokio`, `tempfile`, `sha2`, Unicode normalization, and platform bindings are already present in `nano-verify`'s resolved feature set.

## Pattern Assignments

### `crates/nano-verify/src/climb.rs` (pure state reducer, deterministic transform)

**Primary donor:** `F:/Development/waylandnano/.tmp/upstream-ferrox-factory/src/gate-climb.cts`

**State/default pattern** (`createClimbState`, lines 100-117):

```typescript
const budget = validPositiveBudget ? Math.floor(o.budget) : 12;
const seedN = validSeedWidth ? Math.floor(o.seedN) : 3;
return {
  cheap: stringList(o.cheap), ladder: stringList(o.ladder),
  budget, seedN, calls: 0, phase: 'probe', best: null,
  tried: {}, wins: {}, consolidated: false,
};
```

Port the state shape to the frozen Rust types. Preserve caller model order. Phase-5 authority adds validation before effects: blank or duplicate model ids across `cheap ∪ ladder` produce the frozen `invalid_model_pool` error exit; zero budget takes the frozen `zero_budget` exit; empty cheap is valid and reaches `Exhausted`.

**Strict ratchet pattern** (`betterCandidate`, lines 127-138):

```typescript
if (c.score[0] > b.score[0]) return true;
if (c.score[0] < b.score[0]) return false;
const cSet = new Set(c.fails);
const bSet = new Set(b.fails);
for (const f of cSet) if (!bSet.has(f)) return false;
return cSet.size < bSet.size;
```

Use deduplicated full canonical failure strings. Accept only a strict pass-score increase, or at equal score a genuine strict subset. Never compare failure counts before containment. Pin `{A,B}` versus `{C,D}` and `{A,B}` versus `{C}` as rejects at equal score because neither candidate is a subset.

**Deterministic next-step pattern** (`nextStep`, lines 155-208):

- Probe with `cheap[0]`.
- Ensemble uses the cheap tail, but Phase-5 authority requires truncation to remaining call budget before scheduling.
- Select the first standing check with an untried model.
- Cheap candidates sort wins-descending with original caller order as the stable tie-break.
- Ladder candidates remain in exact caller order and are never win-sorted.
- Surgical `others` is capped at 8; consolidation failures are capped at 10.
- Exactly one consolidation is available; the next plateau stops.

**Immutable fold and accepted-best pattern** (`applyResult`, lines 217-280):

```typescript
next.calls += 1;
// Surgical tries are remembered whether accepted or rejected.
if (step.action === 'surgical') remember(step.target, model);
const accepted = seeding ? true : betterCandidate(candidate, next.best);
if (accepted) {
  next.best = candidate;
  if (!seeding) next.wins[model] += 1;
  next.tried = consolidate ? {} : pruneTo(candidate.fails, next.tried);
}
```

The Rust `StepResult` has no donor compatibility `accepted` override; never add one. Every actually scheduled model call increments `calls` exactly once, including generation failures, but a generation failure has no candidate and never seeds/replaces `best`. Preserve winner identity so the engine returns the accepted artifact rather than the most recently generated artifact.

**Explicit anti-analog:** do not copy `upstream-wcore-0.13.0/crates/wcore-agent/src/orchestration/anvil/climb.rs::evaluate_acceptance` (lines 310-333) or its `RankKey`/`best`. Wcore accepts lateral equal-fail-set moves and severity trades and ranks by fail count/severity/cost. All conflict with CLIMB-02's stricter Ferrox rule.

---

### `crates/nano-verify/src/engine.rs` (parser and async orchestration)

**Primary donor:** `F:/Development/waylandnano/.tmp/upstream-ferrox-factory/src/gate-first-executor.cts`

**Prompt-boundary pattern** (lines 114-159): prompt helpers accept only `spec`, current validated diff bytes, and opaque check ids. Surgical context is target plus at most 8 other ids; consolidation is at most 10 ids. Update the wording to require exactly one raw UTF-8 schema-1 unified diff, no Markdown fences or prose, and at most 16 MiB.

Do not pass `GateInvocation`, inventory records, expected values, gate source/path, baseline `run_artifact`, stdout, evidence, artifact handles, provider errors, or workspace paths into prompt builders. Candidate bytes may appear only as bounded `current_diff` in a later repair prompt.

**Sequential driver pattern** (`runGateFirst`, lines 297-330):

```typescript
for (let guard = 0; guard < 10000; guard++) {
  const step = gateClimb.nextStep(state);
  // ...
  if (step.action === 'ensemble') {
    const results = [];
    for (const model of step.models) {
      results.push(await performBuild(...));
    }
    state = gateClimb.applyResult(state, step, results);
  }
}
```

Keep ensemble generation sequential. Before each scheduled operation recheck budget, cancellation, and the absolute monotonic deadline. No call starts after any has closed. Retain the 10,000-iteration defensive guard even though every non-stop step consumes at least one call.

**Do not port:** donor real-effects factory, mutable filesystem effect, `runGate` effect, temperature/system/max-token fields, `Date.now()`, provider ids/defaults, or QUAL-01 polish (donor lines 340-365). Frozen `Effects` is generation/time/cancellation/event emission only and borrows `&self`.

#### Single schema-1 parser

There is no in-repo parser analog strong enough to copy. Implement the frozen IFACE grammar directly in `engine.rs::parse_candidate_diff` as one complete byte parser:

- exact raw UTF-8 input, LF-only, NUL/CR-free, nonempty, fully consumed;
- maximum 16 MiB inclusive; reject byte 16 MiB + 1 before persistence;
- exact file-record and `---`/`+++` order;
- closed Add/Modify/Delete header pairs and ASCII-normalized paths;
- checked `u64` hunk ranges and exact old/new body counts;
- reject duplicate paths, fences/prose/trailing junk, quoted paths, no-newline markers, binary/mode/link/rename/copy and every other extended header.

`CandidateDiff` follows the in-repo private-field artifact pattern: private path list, exact-byte digest, and private parsed records, with read-only `paths()` and `bytes_sha256()` accessors. Never expose record constructors or raw parsed internals. WP-2 admission and WP-3 installation must reuse this function; do not add a second parser.

#### Read-only expected-change derivation

`derive_expected_changes(&CandidateDiff, starting_root)` consumes the parser's private records rather than reparsing bytes. It computes in memory the exact Add/Modify/Delete postimages, sorted unique normalized paths, `postimage_sha256`, parser-equal `diff_digest`, and a stable digest of the exact validated starting-tree preimages.

Use `crates/nano-verify/src/registry.rs::canonical_json` (lines 53-90) as the digest discipline analog: deterministic ordering, exact bytes, lowercase SHA-256, no ambient or display-string inputs. Use private fields and read-only accessors as specified for `ExpectedChange` and `ExpectedChangeManifest`; external callers cannot construct or mutate either.

For root/path identity defenses, adapt the principles in `crates/nano-protocol/src/acp.rs`:

- `symlink_metadata` prefix inspection around lines 303-329;
- canonical resolution and allowed-root containment around lines 477-495;
- Unix opened-object `dev`/`ino` identity check around line 656;
- Windows reparse-safe, handle-side verification around lines 684-758.

Do not add a dependency on `nano-protocol`; reproduce only the necessary bottom-of-graph platform pattern. Reject noncanonical/link/reparse roots, `.git`, escape, non-regular preimages, Add-over-existing, Modify/Delete-over-absent, hunk/context mismatch, overlap/order errors, and preimage identity changes. Snapshot tests must prove derivation performs no filesystem mutation.

#### Monotonic deadline and cancellation

Use `tokio::time::timeout` only as a local mechanism analog (`crates/nano-verify/src/gate.rs::run_gate`, lines 98-128), but follow the frozen checked-millisecond algorithm exactly:

1. Read only `fx.now_millis()`; never wall clock.
2. `provider_cap = now.checked_add(120_000)`; overflow is `Blocked("deadline_overflow")` / `Error`.
3. `generation_deadline = min(provider_cap, cfg.deadline.monotonic_millis)`.
4. Derive remaining time with checked subtraction; `None` or zero is `TimedOut` / `Error`.
5. Pin and select the generation future against the timer plus a cancellation poll no slower than 50 ms.
6. Drop the losing future; never detach it.
7. Check cancellation before generation, after select, before artifact creation, and before gate execution.
8. Resolve cancellation versus timeout only from `cancellation_requested()` and the monotonic deadline, never provider error text.

Before each gate spawn, checked-subtract the run deadline again and cap a cloned `GateInvocation.timeout` to `min(floored gate timeout millis, remaining millis)`. Reject overflow or zero; do not round a positive sub-millisecond timeout upward and do not mutate the caller's invocation.

#### Closed events and logs

Use the typed-observation shape from `crates/nano-model/src/types.rs::ModelObservation` and `CallHooks::observe` (lines 375-445) as a design analog only: closed enum, typed fields, observer sees no control authority. Do not depend on `nano-model`.

Emit only frozen `EngineEvent` variants/fields. `check_ids` comes from the authoritative inventory and is bounded to its length. Logs/events must never include model/provider ids, prompts, generated bytes, artifact paths, workspace roots, gate invocation/source/path, raw output, expected values, provider error strings, or evidence digests. A provider error is retained only in immediate private control flow and emits a closed `GenerationFailed` observation.

For leakage tests, adapt `crates/nano-agent/src/review_prompt.rs::seed_is_prompt_plus_bundle_only` (lines 316-340): inject distinct canaries for every forbidden source and assert their absence from every prompt, log, event, debug, and serialized outcome while asserting canonical opaque check ids remain visible where allowed.

#### Sealed `ClimbOutcome`

Use the complete terminal enum vocabulary and `is_verified()` predicate from `upstream-wcore-0.13.0/crates/wcore-agent/src/orchestration/anvil/mod.rs` lines 46-86, but use only frozen WP-2 mappings.

The outcome requires the frozen real Rust construction seal: every semantic field is private, `_seal: OutcomeSeal` is private and skipped by serialization, and the sole `pub(crate)` constructor lives in `climb.rs` for use by the `run_climb` exit path. A token used only as a constructor argument would be insufficient if fields were public because a struct literal could bypass it. Provide only the frozen read-only accessors and no public `Default`, constructor, or `Deserialize`. Compile-fail/API tests must prove external literal construction and mutation fail.

The outcome carries the opaque accepted `CandidateArtifact`, not its path or bytes, and never an `ExpectedChangeManifest`. Debug/serialization must not reveal candidate bytes, private workspace/path state, provider text, or prompts.

---

### `crates/nano-verify/src/gate.rs` (trusted bounded execution)

**Direct base pattern:** existing `run_gate` lines 18-128.

Reuse its argv-only spawn, `env_clear` allowlist, process-tree containment, bounded reader, timeout, nonzero-exit-is-not-verdict, and fail-closed parsing. Refactor only through a shared private launcher so the exact landed `run_gate(inv, artifact_path, inventory) -> GateOutcome` API and behavior remain compatible.

#### Opaque workspace and artifact

Implement the frozen private-field shapes directly:

- non-Clone `ArtifactWorkspace` holds `Arc<ArtifactWorkspaceInner>` plus private seal;
- `create_artifact_workspace()` is the only public factory and accepts no path/handle;
- factory reads only `std::env::temp_dir()`, canonicalizes and validates the parent before creating a fresh private child;
- `CandidateArtifact` binds workspace identity, private path, exact-byte SHA-256 and private seal;
- only crate-private `create_candidate_artifact` constructs it after `parse_candidate_diff` succeeds;
- debug output exposes only the digest and is non-exhaustive;
- equality includes `Arc::ptr_eq`, path, and digest;
- exact readback revalidates identity and digest and returns `ArtifactInvalid` on substitution/mutation.

Use `tempfile` lifecycle principles from `crates/nano-verify/src/receipt.rs`'s temp Git fixture and same-directory writer, but do not use a caller directory and do not expose the `TempDir`. Apply the ACP link/reparse/opened-object checks cited above. The product must not hardcode `F:`; F-only TEMP/TMP validation belongs to executor preflight.

#### Evidence-bearing execution

Add `run_gate_execution` and `run_gate_baseline_execution` without replacing legacy `run_gate`.

Use `crates/nano-cli/src/review_diff.rs::drain_capped` (lines 336-354) only as the bounded-stream accounting analog: continue reading to EOF while retaining a strict bound and track completeness explicitly. For the new execution APIs, byte 16 MiB + 1 is `OutputIncomplete`, not compatible truncation. Complete stdout alone earns `log_digest`; stderr is discarded. Record typed `exit_code: None` for spawn, timeout, abnormal/no-code termination, output incomplete, or other non-complete execution as frozen. Candidate evidence always carries the exact generated-byte `artifact_sha256`; baseline evidence intentionally does not.

Reconstruct the full inventory and validate duplicate/unknown failures, exact expected/reported pass and total counts, and Green/Red coherence. New APIs return exact four-field `InconsistentVerdicts`; legacy parse/runner maps it back to landed `InconsistentSummary`. Legacy abnormal termination maps to `NoGateOutput`. Never let execution evidence eligibility depend on exit code alone.

Tests should extend the existing fixture-binary pattern in `tests/gate_contract.rs`: external process output/exit/signal is the oracle, not self-reported state. Add complete/overflow stdout, stderr exclusion, abnormal termination, exact evidence digests, artifact mutation/substitution, inventory/coherence, compatibility mapping, and public-construction compile-fail legs.

---

### `crates/nano-verify/src/lib.rs` and `error.rs`

Follow the existing explicit module and root re-export list in `lib.rs`. Export every frozen public gate/parser/manifest/climb/engine type and function, while leaving constructors, seals, parsed records, workspace internals, and filesystem paths private. Update the crate documentation that currently states WP-2 is excluded.

Use `VerifyError::Artifact(std::io::Error)` for parser, artifact, workspace, and derivation invalid-data/I/O errors exactly as frozen. Do not add `NanoErrorKind`, regenerate the error table, or expose provider errors from `run_climb`.

---

### `UPSTREAM.md`

Follow the existing destination/donor/transformation row format. Add exactly two rows:

- `crates/nano-verify/src/climb.rs` from Ferrox `gate-climb.cts` with strict-ratchet Rust transformation and frozen budget/order deviations;
- `crates/nano-verify/src/engine.rs` from Ferrox `gate-first-executor.cts` plus wcore terminal vocabulary, recording the frozen Effects/parser/manifest/deadline/artifact deviations and omitted QUAL-01 hook.

Amend the existing `gate.rs` row for the additive evidence-bearing APIs, opaque workspace/artifact binding, exact-ms deadline cap, complete-output policy, and OS-temp behavior. Do not add separate parser/manifest provenance rows.

## Shared Patterns

### Fail closed before authority-bearing work

Apply to parser admission, workspace creation, artifact readback, deadline math, gate execution, evidence eligibility, and manifest derivation. Invalid input consumes only the explicitly authorized budget unit; it never creates an artifact, starts a gate, mutates Git, fabricates evidence, or silently downgrades.

### Exact bytes and deterministic ordering

Apply `BTreeMap`/sorted-vector ordering and SHA-256 over exact validated bytes. Preserve file-record order in `CandidateDiff::paths()`, caller order for models, and sorted normalized paths only in the expected-change manifest. Never digest debug/display text.

### Private construction is a security boundary

Opaque workspace, artifact, parsed diff, expected changes/manifest, and climb outcome all require private fields—not merely discouraged constructors. Debug and serialization are explicit allowlists.

### Bounded observation surfaces

Every byte stream, identifier list, prompt context, log, event vector, and timer is bounded by a named contract limit. Completeness is recorded rather than inferred. Truncation never earns evidence.

### No ambient authority

No global provider discovery, model defaults, wall clock, caller workspace path, Git mutation, baseline evidence capture, or receipt construction belongs in WP-2. All allowed authority arrives through frozen typed inputs or is created inside trusted core.

## No Close Analog Found

| Surface | Reason | Planner direction |
|---|---|---|
| Complete schema-1 unified-diff parser | No in-repo parser implements the frozen closed grammar and full-consumption rules. | Implement directly from IFACE; do not adapt TUI display parsers or shell out to Git. |
| Sealed expected-change derivation | No in-repo API computes exact in-memory patch postimages plus base/diff digests without mutation. | Implement directly from IFACE using private parsed records. |
| Opaque no-path `ArtifactWorkspace` | Existing tempdir users accept/know paths or lack the full identity attack model. | Combine frozen shape with ACP opened-object/link defenses; do not expose or accept a root. |
| Engine-owned cancellation select | Existing model hooks are observational and do not exactly implement the frozen deadline/poll/drop contract. | Implement the checked-ms select loop directly and prove losing future drop. |

## Patterns to Avoid

- Wcore severity-Pareto/lateral acceptance or fail-count ranking.
- Parallel ensemble generation.
- Saturating/wrapping deadline arithmetic, seconds conversion, upward rounding, or wall-clock reads.
- Inferring cancellation/timeout from error strings.
- Public artifact/workspace/outcome constructors or all-public struct literals without a private seal field.
- A second diff parser, Git-based parser, or reparsing during expected-change derivation.
- Applying/materializing candidate output or mutating Git in WP-2.
- Returning/logging provider error text, model ids, prompts, candidate bytes, gate internals, paths, or raw output.
- Treating truncated stdout as complete evidence.
- Replacing or changing the source/behavior contract of landed WP-1 `run_gate`.
- Adding provider defaults, production Effects wiring, receipts, Gate Cards, WP-3 CLI, or any WP-5/WP-6/DeepSeek expansion surface.

## Metadata

**Analog search scope:** `crates/nano-verify`, focused security/orchestration analogs under `crates/nano-protocol`, `crates/nano-agent`, `crates/nano-model`, and `crates/nano-cli`; frozen Ferrox and wcore donor snapshots.  
**Primary analogs fully inspected:** Ferrox `gate-climb.cts`, Ferrox `gate-first-executor.cts`, wcore `anvil/climb.rs`, wcore `anvil/mod.rs`, existing `nano-verify` gate/registry/receipt modules.  
**Pattern extraction date:** 2026-08-20  
**Planning readiness:** Complete. Planner must treat frozen authority text as stronger than every analog excerpt.
