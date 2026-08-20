# Phase 05 Validation — WP-2 Gated Climb

## Validation objective

Prove, from behavior rather than structure, that WP-2 implements the frozen pure ratchet, trusted candidate/parser/manifest boundary, complete gate evidence, and injected deadline/cancellation driver while preserving every WP-1 contract. Validation is fail-closed: a zero-test invocation, silent helper omission, unsupported fixture represented as success, stale review identity, dependency drift, or missing platform leg is a failed claim.

The authoritative requirement set is CLIMB-01 through CLIMB-05 in the hash-frozen GOALS/IFACE/WP12 documents. The four authoritative WP-2 tests remain tests 30–33 and must each be discovered exactly once under their fully qualified harness names.

## Canonical F-drive preflight

Every local Cargo, nested downstream-Cargo, fixture, mutation, and gate command begins with this preflight. Product code must not hard-code `F:`; this is executor policy only.

```powershell
$env:TEMP='F:\Temp\Codex'
$env:TMP='F:\Temp\Codex'
$env:CARGO_TARGET_DIR='F:\CargoTarget\wayland-nano'
$repo=(Resolve-Path -LiteralPath ((git rev-parse --show-toplevel).Trim())).Path
$temp=(Resolve-Path -LiteralPath $env:TEMP).Path
$tmp=(Resolve-Path -LiteralPath $env:TMP).Path
$target=(Resolve-Path -LiteralPath $env:CARGO_TARGET_DIR).Path
if(-not $repo.StartsWith('F:\Development\waylandnano\',[System.StringComparison]::OrdinalIgnoreCase)) { exit 1 }
foreach($p in @($temp,$tmp)) {
  if(-not $p.StartsWith('F:\Temp\Codex',[System.StringComparison]::OrdinalIgnoreCase)) { exit 1 }
}
if(-not $target.StartsWith('F:\CargoTarget\wayland-nano',[System.StringComparison]::OrdinalIgnoreCase)) { exit 1 }
```

Abort on failure. In final closure, additionally require the repository, TEMP, TMP, and target directories already exist, their lexical full paths equal their canonical resolved paths, and neither they nor any traversed ancestor carries a junction/reparse/link boundary. Do not fall back to C: or D:. Environment-mutating fixtures hold one poison-tolerant process-wide mutex for the complete fixture lifetime and restore the environment through an RAII guard even on panic. Each temporary downstream crate receives a private child `CARGO_TARGET_DIR` on F:.

## Requirement-to-proof map

| Requirement | Observable behavioral proof | Primary automated evidence | Completion evidence |
|---|---|---|---|
| CLIMB-01 | Probe → budget-truncated sequential ensemble → per-check cheap/ladder surgical escalation → one consolidation → typed stop; accepted artifact/evidence/text move as one identity | exact test 31; exact test 33 real driver scenarios | full helper manifest, crate test, six CI lanes |
| CLIMB-02 | Strict passed-score improvement or equal-score strict canonical failure subset only; no equal-count oscillation or lower-score subset acceptance | exact test 30 plus ratchet mutant receipts | crate test, clippy, six CI lanes |
| CLIMB-03 | Prompts receive only allowed bounded inputs; closed logs/events/outcome/debug/JSON leak no provider/model/gate/path/raw-byte authority; downstream callers cannot forge trusted objects | exact test 33 leakage scenarios; `wp2_public_contract` positive/negative crates | canary-clean scoped capture, six CI lanes |
| CLIMB-04 | Default budget 12; every generation attempt charged; wins count accepts only; model-pool, deadline, cancellation, future-drop, timeout cap, and all terminal mappings are typed | exact tests 32 and 33; scheduler/deadline mutants | repeated process/timer helpers, crate test, six CI lanes |
| CLIMB-05 | Complete parser, manifest, workspace, evidence, compatibility, confinement, API, and driver battery exists and really executes | exact inventory 30–33; helper-manifest/no-zero oracle | `cargo deny check`, `just gate-all`, audit closure, exact-SHA six-lane CI |

## Wave 0 RED contract

Before the corresponding implementation is added, record a RED receipt for each group. A RED receipt contains the phase-base SHA, test commit SHA, exact command, selected test count, exit code, and the missing behavior or violated assertion. Compile/import failures are acceptable only when that wave intentionally introduces the absent module/API; unrelated syntax, dependency-resolution, package-selection, or zero-test output is not valid RED evidence.

| RED group | Required failing behavioral surface | Valid RED command |
|---|---|---|
| W0-A pure ratchet | tests 30 and 32 reject missing strict-subset/call-accounting behavior | enumerate, resolve fully qualified names, run each with `--exact` |
| W0-B scheduler | test 31 rejects missing ordering, truncation, tried-memory, plateau, and identity behavior | fully qualified test 31 with `--exact` |
| W0-C parser/manifest | positive and negative parser tables, digest boundary, sorted manifest, and filesystem no-mutation oracle fail before implementation | focused engine helper filters, each requiring nonzero selected tests |
| W0-D artifact/evidence | workspace/readback attacks and complete-output/inventory fixtures fail before trusted APIs exist | focused gate helper filters, each requiring nonzero selected tests |
| W0-E driver | test 33 scenarios fail for absent driver policy, not merely absent registration | fully qualified test 33 with `--exact --nocapture` |
| W0-F public boundary | downstream negative crates initially compile or positive crate lacks the frozen API, proving the harness has teeth | `cargo test -p nano-verify --test wp2_public_contract -- --nocapture` after the harness itself passes `cargo check --tests` |

After each intended RED is observed, implement only the owning behavior and rerun the identical command GREEN. Never weaken an assertion to manufacture GREEN.

## Exact test discovery and no-zero-test oracle

The phase executor generates a machine-readable manifest from `cargo test -p nano-verify -- --list`. It must contain every pre-existing authoritative WP12 name 1–29 exactly once and these four names exactly once:

- `ratchet_accepts_strict_score_win_and_strict_subset_only`
- `probe_ensemble_surgical_consolidate_path`
- `budget_exhaustion_stops`
- `driver_stub_suite`

For each name, derive the fully qualified name from the line ending `: test`, invoke it with `--exact`, and require output to show `running 1 test` and `1 passed; 0 failed`. Cargo exit zero with `running 0 tests` is fatal. The manifest records `{ordinal, short_name, fully_qualified_name, occurrences, selected, passed, command, head_sha}`. Helpers must never reuse names 1–33.

```powershell
$spec='../../shared/reviews/research-0.2/specs/SPEC-WP12-nano-verify-engine.md'
$inTests=$false
$authoritative=@(Get-Content -LiteralPath $spec | ForEach-Object {
  if($_ -match '^## 7\. Test battery') { $inTests=$true; return }
  if($_ -match '^## 8\.') { $inTests=$false }
  if($inTests -and $_ -match '^\s*(?:[1-9]|[12][0-9]|3[0-3])\.\s+`([^`]+)`') { $matches[1] }
})
if($authoritative.Count -ne 33 -or @($authoritative | Sort-Object -Unique).Count -ne 33) { exit 1 }
$listed = cargo test -p nano-verify -- --list
if($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$wp2=@('ratchet_accepts_strict_score_win_and_strict_subset_only','probe_ensemble_surgical_consolidate_path','budget_exhaustion_stops','driver_stub_suite')
foreach($name in $authoritative) {
  $hits=@($listed | Select-String ("(^|::)"+[regex]::Escape($name)+": test$"))
  if($hits.Count -ne 1) { exit 1 }
  if($name -in $wp2) {
    $full=($hits[0].Line -replace ': test$','')
    $out=@(cargo test -p nano-verify $full -- --exact --nocapture 2>&1)
    if($LASTEXITCODE -ne 0 -or ($out -join "`n") -notmatch 'running 1 test' -or ($out -join "`n") -notmatch '1 passed; 0 failed') { exit 1 }
  }
}
```

## Required helper manifest

Test 33 is an umbrella, not permission to hide coverage. The final helper manifest is populated from actual `--list` output and maps every row below to at least one separately runnable, non-ignored test. Prefixes may be `wp2_`; exact concrete names are frozen in the Plan 02 summary before Wave 3. Each helper command must select at least one test and pass; a missing, ignored-only, or zero-selected helper blocks CLIMB-05.

| Group | Required independently observable helpers |
|---|---|
| Driver | green probe short-circuit; model-pool validation; zero budget; checked deadline arithmetic; cancellation precedence; pending-generation drop; sanitized generation errors; prompt opacity/bounds; invalid candidate no-persist/no-gate; terminal completeness; no manifest/starting-root runtime authority |
| Parser | Modify/Add/Delete; zero/omitted ranges; multi-hunk/record; Unicode body; exact digest; 16 MiB inclusive; every frozen negative encoding/path/header/metadata/range/count/trailing/16 MiB+1 class |
| Manifest | mixed sorted Add/Modify/Delete; exact postimage/diff/base digests; untouched-byte/multi-hunk preservation; no-mutation snapshot; unsafe root/component/volume/object/existence/context/range/identity negatives; parser reuse |
| Gate evidence | Green/Red despite nonzero exit; duplicate/unknown/coherence failures; candidate and baseline four-field `InconsistentVerdicts`; empty inventory; complete stdout/stderr exclusion; exact artifact/log digests; spawn/timeout/abnormal; 16 MiB inclusive/overflow; legacy truncation and reason mapping |
| Workspace | distinct opaque workspaces; non-Clone boundary; transfer/drop lifetime; cleanup; exact readback/hash; Debug/serialization non-disclosure; traversal/root/link/reparse/mount/volume/nonregular/move/root-substitution/byte-mutation attacks; no spawn after invalidity |
| Downstream API | supported positive crate; each literal-construction, mutation, constructor, clone, stale-variant, wrong-argument, and manifest/root-smuggling negative case with intended diagnostic |

Platform-unavailable alternate-volume/reparse fixture construction is recorded as a named WARNING with capability evidence; it is never recorded as a pass. The generic confinement oracle must still execute and pass on every lane. Any actual behavioral assertion failure is a BLOCKER.

## Wave/task validation matrix

All commands below run after the F-drive preflight and fail immediately on a nonzero native exit.

| Wave / plan / task | Behavioral checkpoint | Exact automated command |
|---|---|---|
| Wave 1 / 05-01 T1 | Tests 30–32 exact once; pure state/scheduler GREEN; no effectful authority | exact discovery loop for tests 30–32; `cargo test -p nano-verify`; `cargo clippy -p nano-verify --all-targets -- -D warnings` |
| Wave 2 / 05-02 T1 | Sealed workspace/artifact and complete candidate/baseline evidence; WP-1 compatibility | `cargo test -p nano-verify gate -- --nocapture`; helper-manifest gate/workspace rows; `cargo test -p nano-verify --test gate_contract -- --nocapture`; clippy |
| Wave 2 / 05-02 T2 | Sole fully consuming parser and read-only sealed manifest | `cargo test -p nano-verify engine -- --nocapture`; helper-manifest parser/manifest rows; clippy |
| Wave 2 / 05-02 T3 | Exact test 33 and every driver helper; full WP-1 regression | exact discovery/run for `driver_stub_suite`; helper-manifest driver rows; `cargo test -p nano-verify`; clippy |
| Wave 2 / 05-03 T1 | Harness compiles independently; supported API compiles; every forbidden API use fails for intended reason | `cargo check -p nano-verify --tests`; `cargo test -p nano-verify --test wp2_public_contract -- --nocapture`; reject zero-test/dependency-resolution false positives |
| Wave 3 / 05-04 T1 | Final facade exactly supports frozen getters/functions and preserves opacity | public-contract target; all exact tests 30–33; `cargo test -p nano-verify`; clippy |
| Wave 3 / 05-04 T2 | Exactly two new provenance rows + bounded gate-row amendment; no manifest/lock/dependency drift | `git diff --exit-code 7bcbc12fec0624aacbc3953e4f2c7d1a2c4414e0 -- Cargo.toml crates/nano-verify/Cargo.toml Cargo.lock`; `cargo deny check`; `git diff --check` |
| Wave 3 / 05-04 T3 | The frozen 38-ID/operator/test set is present with exact family counts; every selected exact test kills its mutant for the intended assertion; pristine bytes are restored exactly | validate `05-MUTATION-RECEIPTS.json`; require exact ID-set equality, exact deny-unknown fields, exact operator/test mapping, family counts 4/7/3/6/6/5/7, one shared committed `head_sha`, frozen base, exact recorded command, `selected_count=1`, RED diagnostic exclusions, GREEN zero, and pristine/restored/live blob equality; `git diff --check` |
| Wave 4 / 05-05 T1 | One identity-bound Critical/High audit, zero or one consolidated fix, independent final recheck | validate `05-REVIEW.md` base/head/diff hash/fix count/finding severities/recheck identity against Git; unresolved Critical/High or second fix round blocks |
| Wave 4 / 05-05 T2 | Builder-local merge readiness, exact scope, clean canary, no promotion overclaim | all exact names; all helper rows; public contract; full crate; clippy; deny; repeated process fixtures; `just gate-all`; ownership/dependency/generated-table/canary checks |
| Wave 4 / 05-05 T3 | After Tasks 1–2 and summary are committed, write uncommitted `05-PROMOTION-REQUEST.json` bound to that immutable `product_head/product_tree`; validate it while it is the sole dirty path, then commit only it as externally derived `builder_tip` and make no later builder change | require exact top-level fields `schema,base_sha,product_head,product_tree,workflow_name,workflow_path,required_jobs,integration_expectations`; schema 1, frozen base, current committed product HEAD/tree, exact workflow/path/jobs, and exact expectation keys `builder_tip_protocol,ci_query,integration_gate,master_tip,merge_parents,request_only_diff`; JSON contains no own future hash, run ID, integration SHA, or CI result |

## Mutation-strength receipts

Mutation proofs run one at a time against final pre-audit source, using exact saved owned-file bytes for restoration and never a broad checkout/reset. `05-MUTATION-RECEIPTS.json` contains exactly 38 deny-unknown receipt objects with exactly `id,family,operator,source_path,base_sha,head_sha,pristine_blob_sha,command,intended_test,selected_count,red_exit_code,red_diagnostic,restored_blob_sha,green_exit_code,timestamp`. All rows bind base `7bcbc12fec0624aacbc3953e4f2c7d1a2c4414e0` and one shared pristine pre-audit `head_sha` that resolves as a commit. Every receipt discovers the frozen fully qualified test exactly once, records `selected_count=1`, and records exactly `cargo test -p nano-verify <intended_test> -- --exact --nocapture`. RED must be nonzero with a nonempty assertion-specific diagnostic that does not match `running 0 tests`, `could not compile`, or `environment`; the identical command then runs GREEN. Restoration requires `pristine_blob_sha == restored_blob_sha == git hash-object source_path`.

| Family / count / frozen intended test | Exact frozen IDs and operators |
|---|---|
| ratchet / 4 / `climb::tests::ratchet_accepts_strict_score_win_and_strict_subset_only` | `R01 strict_gt_to_gte`; `R02 compare_fail_count_only`; `R03 subset_to_nonstrict`; `R04 omit_subset_containment` |
| scheduler / 7 / `climb::tests::probe_ensemble_surgical_consolidate_path` | `S01 clear_tried_on_every_accept`; `S02 retain_cleared_check_keys`; `S03 sort_ladder_by_wins`; `S04 lexical_cheap_tiebreak`; `S05 schedule_full_ensemble_past_budget`; `S06 allow_second_consolidation`; `S07 replace_artifact_keep_old_evidence` |
| budget / 3 / `climb::tests::budget_exhaustion_stops` | `B01 charge_only_gated_candidates`; `B02 schedule_at_calls_equal_budget`; `B03 increment_wins_on_reject_or_generation_failure` |
| parser / 6 / `engine::tests::wp2_candidate_parser_matrix` | `P01 accept_crlf`; `P02 ignore_trailing_lines`; `P03 dedupe_duplicate_path`; `P04 unchecked_range_parse`; `P05 hash_normalized_not_exact_bytes`; `P06 allow_dev_null_pair_mismatch` |
| manifest / 6 / `engine::tests::wp2_expected_change_manifest_matrix` | `M01 write_during_derivation`; `M02 preserve_record_order_not_sorted`; `M03 omit_preimage_from_base_digest`; `M04 accept_add_over_existing`; `M05 reparse_bytes_in_derivation`; `M06 allow_overlapping_hunks` |
| gate / 5 / `engine::tests::wp2_gate_execution_evidence_matrix` | `G01 exit_code_controls_verdict`; `G02 digest_stderr`; `G03 retain_digest_after_overflow`; `G04 expose_new_reason_through_legacy_enum`; `G05 trust_pre_mutation_artifact_hash` |
| driver / 7 / per-row exact helper | `D01 unchecked_or_saturating_deadline` → `engine::tests::driver_deadline_arithmetic_is_checked`; `D02 timeout_precedes_cancellation` → `engine::tests::driver_cancellation_precedes_timeout`; `D03 detach_losing_generation_future` → `engine::tests::driver_pending_generation_is_cancel_safe`; `D04 leak_gate_or_provider_canary` → `engine::tests::driver_prompts_are_opaque_and_bounded`; `D05 accept_blank_or_duplicate_model` → `engine::tests::driver_model_pool_validation_precedes_effects`; `D06 zero_budget_schedules_generation` → `engine::tests::driver_zero_budget_is_typed`; `D07 incomplete_terminal_mapping_or_verified_predicate` → `engine::tests::driver_terminal_mapping_is_complete` |

The table is an exact per-ID mapping, not descriptive guidance. The verifier compares the complete `(id,family,operator,intended_test)` tuple set for equality with these 38 rows in addition to exact ID equality and family counts; swapping operators or tests between valid IDs fails. There may be no duplicates, substitutions, aggregation, or extras. A survivor, wrong-reason failure, zero-test invocation, compile/environment failure, command/head mismatch, dirty restore, or live-hash mismatch blocks the audit. Unkilled Critical/High mutants are audit findings, not deferred debt.

## Process, timer, and race repetition

After focused GREEN, repeat the resource-sensitive helper set enough to expose process saturation, descendant-reaping, fixture-env, move/substitution, cancellation/drop, and nested-Cargo races. Use readiness channels and bounded drop guards; do not add sleeps as correctness or weaken timing assertions. The final summary records iteration count and per-command pass count. Any flake is a failure requiring diagnosis within the one authorized fix round.

## Full local closure

The final builder sequence is:

1. F-drive preflight.
2. Enumerate all 33 authoritative WP12 names and require exact-once inventory.
3. Execute exact tests 30–33 with the no-zero oracle.
4. Execute every concrete helper-manifest row with a nonzero selected-test count.
5. `cargo test -p nano-verify --test wp2_public_contract -- --nocapture`.
6. `cargo test -p nano-verify`.
7. `cargo clippy -p nano-verify --all-targets -- -D warnings`.
8. `cargo deny check`.
9. Prove root/crate manifests and `Cargo.lock` unchanged from phase base.
10. Run repeated native process/timer/confinement fixtures.
11. `just gate-all`.
12. Prove exact ownership, no nested `.git`, no generated error-table delta, and no WP-3+ or expansion surface.
13. Canary-scan the exact changed product/provenance/planning-review include list through the opaque F-drive key path without reading, printing, hashing, or persisting the key value; require zero hits and exact file-set equality.

## Integrator-owned Landed gate — outside builder plans

Builder-local GREEN is not CI GREEN. After Tasks 1–2 and `05-05-SUMMARY.md` are committed, Plan 05 Task 3 captures that immutable committed identity as `product_head` and `product_tree`. While HEAD still equals `product_head`, it writes uncommitted `.planning/phases/05-wp-2-gated-climb/05-PROMOTION-REQUEST.json` with exactly `schema,base_sha,product_head,product_tree,workflow_name,workflow_path,required_jobs,integration_expectations`: schema 1; frozen base `7bcbc12fec0624aacbc3953e4f2c7d1a2c4414e0`; current product HEAD/tree; exact workflow/path; the exact six unique job names below; and expectation keys exactly `builder_tip_protocol,ci_query,integration_gate,master_tip,merge_parents,request_only_diff`. Validation requires this untracked request to be the sole dirty path. It is then committed alone; that future commit is `builder_tip`, but neither its SHA nor its tree appears inside the JSON. No file, including the summary, changes after that request-only commit. The builder never merges, pushes, touches detached integration, or queries/claims CI.

L is a separate integrator-owned promotion gate outside Plans 01–05. The integrator must fetch the remote, derive the expected integration identity from `origin/master`, create the detached no-ff merge, and prove the landed commit rather than accepting a supplied SHA:

1. Read `product_head/product_tree` from the request and prove both resolve to those exact committed bytes.
2. Derive `builder_tip` from the final builder branch tip; require it has exactly one parent equal to `product_head`.
3. Require `git diff --name-only product_head..builder_tip` to equal only `.planning/phases/05-wp-2-gated-climb/05-PROMOTION-REQUEST.json`, validate the request blob/schema at `builder_tip`, and prove no later builder commit exists. The request cannot self-report `builder_tip`.
4. Record pre-merge `origin/master`, then create the detached no-ff merge using exact `builder_tip` as the second parent; run full `just gate-all`, push, and fetch again.
5. Require the fetched `origin/master` commit to equal the expected integration SHA.
6. Require `git rev-list --parents -n 1 <integration_sha>` to contain exactly three tokens: the integration commit plus exactly two parents.
7. Require parent 1 to equal the recorded pre-merge `origin/master`, parent 2 to equal derived `builder_tip`, and neither parent to equal the merge commit. A self-referential request, extra post-request commit, non-request diff, fast-forward, octopus merge, reversed/unrelated builder parent, local-only commit, or remote divergence blocks L.

Only after the remote no-ff identity and integration gate are proven does the integrator authenticate the resulting Actions run directly:

```powershell
$repo=(gh repo view --json nameWithOwner | ConvertFrom-Json).nameWithOwner
$api=gh api "repos/$repo/actions/runs/$runId" | ConvertFrom-Json
$view=gh run view $runId --json databaseId,headSha,workflowName,status,conclusion,jobs,url | ConvertFrom-Json
```

The API and run-view objects must agree that workflow name is exactly `wayland-nano-gate`, workflow path is exactly `.github/workflows/gate.yml`, `head_sha`/`headSha` equals the exact integration commit, and both status/conclusion pairs are `completed`/`success`. The live `jobs` array must contain exactly these six names and no others:

| Exact GitHub job name | WP-2 mandatory platform behavior |
|---|---|
| `gate (windows-latest, x64)` | job-object timeout/descendant reap, case-insensitive environment, junction/reparse and same-volume identity, replacement/move/abnormal termination |
| `gate (windows-11-arm, arm64)` | same Windows battery under ARM resource pressure; serialized nested Cargo/process fixtures |
| `gate (macos-14, arm64)` | process-group timeout, symlink/mount rejection, signal/no-code termination, canonical temp behavior |
| `gate (macos-15-intel, x64)` | same macOS battery and exact parser/digest/public API suite |
| `gate (ubuntu-22.04, x64)` | process-group timeout/descendant reap, symlink/mount rejection, signal/no-code, file-identity race |
| `gate (ubuntu-24.04-arm, arm64)` | same Linux battery under ARM resource pressure; serialized process/compile fixtures |

Each live job must itself be `completed`/`success`. The authenticated Landed evidence may live in an external integrator record or integrator summary; it must be derived from those query objects and bind `run_id,integration_sha,workflow_name,workflow_path,status,conclusion,url,jobs` without hand-entered status fields. It must equal the live query on database ID, the fetched `origin/master` integration SHA, workflow identity/path, run status/conclusion/URL, exact six-job set, and every job's status/conclusion. A missing, extra, queued, skipped, cancelled, failing, mixed-workflow, mismatched-SHA, wrong-parent, or unpushed commit blocks Phase 6. A rerun is new evidence and must independently satisfy the same live-query oracle.

## Evidence-state ledger

Evidence is reported honestly in three independent states:

| State | Meaning | Required artifacts | Forbidden claim |
|---|---|---|---|
| I — Implemented | Builder branch behavior and local gates are green on exact committed bytes | RED/GREEN receipts, exact/helper manifests, mutation receipts, summaries, final local commands | merged, pushed, CI green, promoted |
| R — Reviewed | Binding Critical/High review matches exact base/HEAD/diff, uses ≤1 fix round, and independent recheck has zero unresolved Critical/High | `05-REVIEW.md`, identities, diff hash, fix/recheck chain | landed or CI green |
| L — Landed | Integrator derives a request-only `builder_tip` whose sole parent is request `product_head`; fetched `origin/master` is the expected detached no-ff merge with exactly two parents (pre-merge remote tip + derived builder tip); integration full gate passed; live GitHub API/view proves exact-SHA workflow is literal 6/6 | product head/tree proof, builder-tip parent and sole-path diff proof, remote tip derivation, merge SHA/ordered parents, push/fetch evidence, post-merge gate receipt, authenticated run and six job conclusions in an external integrator record/summary | completion based on a self-referential request, unproved/follow-on builder tip, local-only merge, wrong parents, manually entered status, or stale/superseded CI |

`05-05-SUMMARY.md` and committed `05-PROMOTION-REQUEST.json` may claim I and R only. L is recorded separately by the owner/integrator after remote no-ff identity, integration gate, push/fetch, and exact-SHA CI authentication. The phase is complete only when all three are evidenced; until then Phase 6 does not start.

## Failure and escalation rules

- Behavioral failure after test-fixture correction is a BLOCKER; do not edit requirements or weaken assertions.
- Zero selected tests, ignored-only helpers, dependency-resolution diagnostics, unsupported capability hidden as success, or incomplete six-lane CI is unverified, never green.
- Platform-unavailable attack fixture construction is a named WARNING only when the generic confinement oracle passes; implementation refusal failure remains a BLOCKER.
- Any required dependency/feature, generated-table, other-crate, `.github`, Git/materializer/receipt/CLI, WP-3+, WP-5/WP-6, DeepSeek, profile, memory, MCP, or external-agent change is an authority/scope deviation and blocks handoff.
- The builder never merges, pushes, touches detached integration, claims CI success, or self-promotes.
