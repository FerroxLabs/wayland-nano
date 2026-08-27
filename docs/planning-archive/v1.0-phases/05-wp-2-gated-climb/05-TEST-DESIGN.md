# Phase 5 / WP-2 Test Design

Status: preimplementation contract. Authority: `SPEC-WP-INTERFACES.md`,
`SPEC-WP12-nano-verify-engine.md` section 7, and `GOALS.md` WP-2 as frozen in
`7bcbc12`. This document does not relax the requirement that all four authoritative
test names 30--33 exist exactly.

## Test topology

Keep contract ownership visible in the source tree:

| Surface | Location | Test form |
|---|---|---|
| Pure ratchet and scheduler | `src/climb.rs` `#[cfg(test)]` | synchronous unit tests; no clock, filesystem, process, or provider fixture |
| Driver policy, candidate parser, manifest derivation | `src/engine.rs` `#[cfg(test)]` | one authoritative Tokio umbrella plus focused synchronous/async helpers |
| Candidate/baseline execution and workspace confinement | `src/gate.rs` `#[cfg(test)]` | crate-private behavioral helpers (required to reach the sole candidate constructor) |
| Public opacity and source compatibility | `tests/wp2_public_contract.rs` plus temporary downstream crates | external integration and compile-fail tests; no access to crate-private helpers |

Do not put clocks, cancellation, process launch, candidate I/O, or provider errors in
test 31. Do not make test 33 one untraceable monolith: the exact named Tokio test calls
focused scenario functions whose names appear below. All fixtures are generated under
the OS temporary directory; no repository files or Git state are mutated.

## Authoritative tests 30--33

### 30. `ratchet_accepts_strict_score_win_and_strict_subset_only`

Table-drive `better_candidate` with sealed test candidates and assert:

1. `best == None` accepts the first valid candidate.
2. `(3,4)` accepts over `(2,4)` even if its failure set is not a subset.
3. Equal score accepts only a deduplicated strict subset: `{A}` over `{A,B}`.
4. Equal score rejects equal sets, including duplicate/reordered representations.
5. Equal score rejects `{C,D}` against `{A,B}` in both directions; fold both attempts
   through `apply_result` and prove the winner identity never ping-pongs.
6. Lower passed score rejects even if the failure set is a strict subset.
7. Equal passed but different totals does not create a third acceptance rule; only the
   score tuple comparison defined by the implementation contract and strict subset rule
   may accept.

Teeth mutants: replace `>` with `>=`; compare `fails.len()`; use non-strict subset;
compare only failure counts. Each mutant must make at least one row red.

### 31. `probe_ensemble_surgical_consolidate_path`

Build `ClimbState` directly and advance only through `next_step`/`apply_result`.
Use caller order `cheap=[c0,c1,c2,c3]`, `ladder=[l0,l1]`, `seed_n=4`, and a budget
that permits the complete path. Script identity-bound `StepResult`s as follows:

| Step | Result purpose | Required observation |
|---|---|---|
| probe `c0` | seeds red `{A,B,C}` | phase becomes Ensemble; calls = 1 |
| ensemble | remaining cheap tail, with budget cap exercised in a second small state | models preserve caller order; best artifact is the strict winner, not last result |
| surgical target `A` | `c2` loses, `c1` accepts `{B,C}` | attempted model remembered; accept increments only winner model; tried keys pruned to still-failing checks |
| same target | exhaust all cheap for that target | ladder is unavailable until this point |
| escalation | `l0`, then `l1` in caller order | ladder ignores wins and lexical order |
| plateau | one Consolidate using best-track-record cheap model and at most first 10 failures | consolidation accept resets the entire tried map and preserves winner artifact identity |
| second plateau | no untried model remains | exact `Stop { Plateau }`; no second consolidation |

Add two contained scheduler subcases within the same named test:

- Seed truncation: budget has two calls remaining and cheap tail has three models;
  `Ensemble.models` contains exactly the first two.
- Ordering: preload unequal cheap wins and prove wins-descending ordering; preload equal
  wins and prove stable original caller order. Give ladder models inverse/high wins and
  prove their order remains exactly caller order.

Assert every fold preserves immutable input state, calls equal the number of supplied
results, rejected candidates never replace `best`, accepted artifact/evidence/text move
together, surgical attempts are remembered win or lose, and generation-error-shaped
results (no artifact/evidence) consume a call without seeding or replacing best.

Teeth mutants: globally clear tried on every accept; retain cleared checks; sort ladder
by wins; lexical tie-break; schedule full ensemble past budget; consolidate twice;
replace accepted artifact but retain old evidence.

### 32. `budget_exhaustion_stops`

Use budget 3 and deterministic red candidate results. Drive probe plus remaining legal
steps until `Stop { Budget }`; assert exactly three results/calls, no fourth model is
returned, best remains the highest ratcheted candidate, and `wins.values().sum()` equals
the number of accepted replacements (not calls, gates, or green checks). Include a
generation failure among the three to prove it consumes budget but never wins.

Teeth mutants: increment calls only for gated candidates; permit `calls == budget` to
schedule; increment wins on reject/generation failure.

### 33. `driver_stub_suite`

One `#[tokio::test]` invokes the following scenario functions and awaits each one. The
stub `Effects` uses interior synchronization and records ordered generate calls,
prompts, closed events, clock reads, and cancellation reads. Gate execution is exercised
through real fixture subprocesses; the effects seam never supplies a gate outcome.

| Scenario helper | Behavioral assertion |
|---|---|
| `driver_green_probe_short_circuits` | one caller-first model call; Green maps to `Verified` + `Solved`; rounds 1; winner exact bytes readable |
| `driver_model_pool_validation_precedes_effects` | blank/trim-empty or duplicate across pools => `Blocked("invalid_model_pool")`, Error, zero calls; empty cheap => Exhausted; no built-in identifiers |
| `driver_zero_budget_is_typed` | zero budget returns the frozen zero-budget terminal/stop mapping, emits only final closed stop records, schedules nothing |
| `driver_deadline_arithmetic_is_checked` | absolute monotonic millis uses checked add/subtract; expired entry, exact boundary, overflow, between-operation expiry, sub-ms floor, and gate-timeout min(remaining, configured) |
| `driver_cancellation_precedes_timeout` | cancellation wins when both observable; no later effect/event except closed stop |
| `driver_pending_generation_is_cancel_safe` | never-resolving generate completes within deadline/cancel polling bound; losing future drop guard fires before return; no detached task, artifact, or gate |
| `driver_generation_errors_are_bounded_and_sanitized` | failed generation consumes one call, may continue, never seeds best; provider canary absent from outcome/debug/JSON logs/events/prompts |
| `driver_prompts_are_opaque_and_bounded` | prompts include raw UTF-8 unified-diff/no-fence/16-MiB instructions and only spec/current diff/opaque inventory ids; canaries for argv, gate path/source, expected values, baseline artifact, provider/model metadata absent |
| `driver_invalid_candidate_never_persists_or_gates` | parser rejection consumes a call, creates no candidate file, invokes no gate; valid candidate is parsed before persistence |
| `driver_terminal_mapping_is_complete` | every `StopReason` maps to required terminal; `is_verified()` true only for Verified; every public log/event uses closed enums and bounded ids |
| `driver_outcome_carries_no_manifest_or_starting_root` | runtime/public API admits only workspace by value and returns winner artifact, never `ExpectedChangeManifest` or a root |

Every scenario asserts bounded `logs` and `events`, exact `rounds_used`, the final
`Stopped` record, and no leakage through `Debug` or `serde_json` output.

## Required focused helper matrix

These helpers are part of test 33's required coverage but should remain separately
runnable. Their exact names may be prefixed with `wp2_`; do not reuse names 1--29.

### Candidate parser

Positive table: Modify, Add, Delete; zero-count legal ranges; omitted count; multiple
hunks; multiple records; `+`/`-` data that resembles a header; Unicode hunk body; exact
16 MiB valid diff. Assert record-order `paths()` and independently computed lowercase
SHA-256.

Negative table (one case per rejection class): empty, invalid UTF-8, NUL, CRLF,
unterminated final line, trailing blank/prose/fence, malformed/extra extended header,
quoted or mismatched paths, absolute/drive/UNC/backslash/space/control/empty-component/
`.`/`..`/`.git` path, duplicate record, invalid OLD/NEW pairing, zero-hunk record,
binary/index/mode/symlink/submodule/rename/copy/no-newline marker, malformed/overflow
range, invalid zero range, empty hunk body, count mismatch, unconsumed line, and
16 MiB + 1. Every rejection is `VerifyError::Artifact(InvalidData)`.

Mutants: accept CRLF; ignore trailing lines; dedupe repeated path; unchecked integer;
hash normalized rather than exact bytes; allow `/dev/null` pairing mismatch.

### Expected-change manifest

Materialize a canonical detached-tree directory and parse a single fixture containing
Add/Modify/Delete, with Modify split across multiple hunks. Assert sorted unique entries,
exact kinds, independently hashed postimages, Delete-only `None`, parser-equal diff
digest, stable base-tree digest, untouched bytes preserved, and byte-for-byte filesystem
snapshot unchanged after derivation.

Negative table: relative/noncanonical root; linked directory/root (Unix symlink and
Windows junction/reparse fixture); alternate volume when available; `.git`; escape;
Add-over-existing; Modify/Delete absent; non-regular preimage; context mismatch;
overlap/out-of-order/impossible ranges; preimage identity change. Platform-unavailable
alternate-volume/reparse construction is an explicit capability-detected WARNING, not a
silent pass; the generic confinement case must still run everywhere.

Mutants: filesystem write during derive; sort by record order; omit preimage from base
digest; accept Add-over-existing; independently reparse bytes; allow overlapping hunks.

### Gate execution and evidence

Real fixture modes cover: Green and Red despite nonzero exit; duplicate/unknown FAIL;
structural and Green/Red coherence mismatch yielding the exact four-field
`InconsistentVerdicts` from candidate and baseline APIs; empty inventory; complete
stdout digest; stderr excluded from digest; candidate artifact SHA mandatory and exact;
spawn error and timeout with `exit_code/log_digest == None`; abnormal signal/no code;
16 MiB complete boundary; overflow => `OutputIncomplete` with both optional evidence
fields None. Re-run compatibility `run_gate` against the same fixtures to prove its
16-MiB prefix truncation and landed reason mappings (`InconsistentSummary`, abnormal to
`NoGateOutput`) have not changed.

Mutants: treat exit code as verdict; digest stderr; return digest after overflow; map new
coherence error directly into legacy enum; accept artifact hash computed before a byte
mutation.

### Workspace/candidate confinement

Set `TEMP`/`TMP` canonically beneath the test's F-drive temp root in executor preflight,
then call only zero-argument `create_artifact_workspace`. Prove non-Clone opacity,
distinct workspaces, survivor readback after workspace transfer/drop, exact byte hash,
cleanup after last artifact handle, and Debug/serialization non-disclosure. Crate-private
attack fixtures exercise traversal, root equality, symlink/junction/reparse component,
move/rename substitution, stored-root substitution, alternate volume, non-regular file,
and post-creation byte mutation; all fail `ArtifactInvalid` and never spawn a gate.

Run environment-mutating tests under one poison-tolerant static mutex for their complete
fixture lifetime. Restore environment in an RAII guard even on panic.

### External compile-fail teeth

Use temporary downstream crates with `nano-verify = { path = <canonical repo crate> }`
and `cargo check --offline`. Serialize them with a static mutex and give each a private
F-drive `CARGO_TARGET_DIR`. Assert failure diagnostics name the intended privacy/type
boundary, not dependency resolution. Cases:

1. literal construction and field mutation of `CandidateArtifact`;
2. call to `create_candidate_artifact` from downstream code;
3. literal construction/mutation of `ArtifactWorkspace` and attempted clone;
4. literal construction/mutation of `CandidateDiff`, `ExpectedChange`, and
   `ExpectedChangeManifest`;
5. literal construction of `ExecutionGateOutcome::FailClosed` with a stale
   three-field `InconsistentVerdicts`;
6. passing bytes/path to `derive_expected_changes` instead of `&CandidateDiff`;
7. passing a starting root/manifest to `run_climb` or extracting a manifest from
   `ClimbOutcome`.

A companion positive downstream crate must compile the supported getters, parser,
manifest getters, zero-argument workspace factory, and landed path-based `run_gate`.

## Red implementation sequence

1. Add exact tests 30 and 32 against the public pure skeleton; verify RED because
   `climb.rs`/exports do not exist. Implement types, `better_candidate`, call accounting,
   and budget stop until these two alone are green.
2. Add exact test 31; verify RED on scheduler transitions. Implement probe/ensemble,
   stable win ordering, per-target tried memory, ladder ordering, one consolidation,
   and identity-bound folding until green.
3. Add parser positive/negative tables; verify RED. Implement the single schema-1 parser
   and sealed parsed representation until green.
4. Add manifest positive/no-mutation tests, then negative confinement/application cases;
   implement derivation only after each RED group is observed.
5. Add candidate workspace/readback and external compile-fail tests; verify constructor,
   field, and root attacks are RED before sealing.
6. Add candidate/baseline evidence fixtures beside the existing WP1 compatibility
   tests; implement the shared capture core while running legacy tests on every step.
7. Add exact test 33 with the green probe, then layer model validation, generation
   failures, prompt/events, deadline/cancellation, and terminal completeness one RED
   scenario at a time.
8. Run each authoritative test by exact name, all nano-verify tests, cross-target clippy,
   then `just gate-all`. Do not declare a gap filled unless the named command actually
   ran green.

## Commands and platform lanes

Focused native commands (with `TEMP`, `TMP`, and `CARGO_TARGET_DIR` already proven on
F: by the executor):

```text
cargo test -p nano-verify ratchet_accepts_strict_score_win_and_strict_subset_only -- --exact
cargo test -p nano-verify probe_ensemble_surgical_consolidate_path -- --exact
cargo test -p nano-verify budget_exhaustion_stops -- --exact
cargo test -p nano-verify driver_stub_suite -- --exact
cargo test -p nano-verify
cargo clippy -p nano-verify --all-targets -- -D warnings
just gate-all
```

Because unit-test names may be module-qualified, first confirm exact discovery with
`cargo test -p nano-verify -- --list`; use the fully qualified printed name with
`--exact` in evidence.

CI platform matrix:

| Lane | Mandatory cases |
|---|---|
| Windows x64/ARM64 | Job-object timeout/descendant reap, case-insensitive env, junction/reparse rejection, same-volume identity, replacement/move attack, abnormal termination |
| Ubuntu x64/ARM64 | process-group timeout/descendant reap, symlink/mount rejection, signal/no-code termination, file identity race |
| macOS ARM64 | process-group timeout, symlink/mount rejection, signal/no-code termination, canonical temp-root behavior |
| All | parser limits, exact digests, pure tests 30--32, driver clocks/cancellation, public compile-fail/positive crates, legacy WP1 regression suite |

Timing assertions use bounded channels/drop guards and generous outer bounds; they do
not sleep to infer correctness. Process fixtures report readiness before timeout logic
starts. Resource-heavy subprocess/compile fixtures are serialized for their entire
lifetime to avoid the ARM process-saturation failures already observed in WP1 CI.

## Completion evidence map

| Requirement | Required evidence |
|---|---|
| CLIMB-01 | test 31 exact sequence plus test 33 real driver path |
| CLIMB-02 | test 30 oscillation/subset matrix and identity-preserving fold |
| CLIMB-03 | prompt/event/log leakage canaries, private-field compile fails, Debug/JSON checks |
| CLIMB-04 | test 32 exact accounting; deadline/cancel/model-pool/terminal helpers |
| CLIMB-05 | all four exact named tests, parser/manifest/execution/confinement helpers, full platform matrix |

No helper may silently skip a required behavior. Unsupported platform fixture creation
is reported as a WARNING with the generic cross-platform confinement oracle still green;
an assertion failure in the implementation is a BLOCKER, not grounds to weaken the test.
