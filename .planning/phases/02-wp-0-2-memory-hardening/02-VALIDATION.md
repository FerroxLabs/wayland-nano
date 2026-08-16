# Phase 02 Validation — WP-0.2 Memory Hardening

## Validation objective

Prove each MEM requirement at the narrowest reliable layer and retain enough exact evidence to distinguish measurement, correction eligibility, B1 acceptance, and the legitimate neither terminal outcome. Every command below is PowerShell 5.1-safe and fails immediately on a nonzero native exit.

## Nyquist requirement map

| Requirement | Observable proof | Automated gate | Evidence |
|---|---|---|---|
| MEM-01 | Exact closed NDJSON schema; 25-turn append cadence; inert feature/default states; no ACP stdout/stderr records; configured sink and later-write failures follow the signed policy; PWS and size accounting work; sessions_map is 0/1 | `cargo test -p nano-cli acp_mode::tests::mem_stats --features mem-stats` plus default `cargo check -p nano-cli --all-targets` | Focused test output and exact `run-<id>/mem-stats.ndjson` |
| MEM-02 | One 900-second run correlates retained fields with external PWS per PID using the signed quantitative threshold and tie rule | Combined-feature release build, `node scripts/soak/test-budgets.mjs`, strict exact-path NDJSON parser | `run-<id>/WP-0.2-PROFILE-DECISION.md`, mem-stats, samples, manifest |
| MEM-03 | Exactly one fold/tool/measured-neither arm is signed only after a completed manifest and sufficient correlation; wrapper abort remains unclassified | Classified-state and exact decision-path grep plus baseline diff audit | Signed classified decision, or blocked aborted/unclassified audit record |
| MEM-04 | Fold arm preserves full-rebuild equivalence and bounds auxiliaries; tool arm proves reuse/invalidation/hydration; neither makes no correction | Existing equivalence oracle, new selected-arm regression, `cargo test -p nano-agent` | Test logs and selected-arm diff/no-op record |
| MEM-05 | Eligible correction completes 3600 seconds and all B1 absolute/end-ratio/slope checks pass; B11 stays separate; ineligible paths make no acceptance claim | Existing `evaluateB1` unit gate plus exact-run manifest/sample evaluation | `run-<id>/WP-0.2-B1-ACCEPTANCE.md`, manifest, sample digest, F-45 disposition |

## Wave 0 tests required before implementation

1. Serialization accepts exactly the fifteen locked fields and rejects unknown fields.
2. Default build has no reporter side effects; feature-enabled/no-env remains inert.
3. Enabled reporter appends independently parseable lines at completed turns 25 and 50 after fold advancement.
4. Reporter records never appear on ACP stdout or stderr; configured sink and later write errors match the signed DEV-WP-0.2A policy.
5. Windows PWS is nonzero; unsupported platforms cannot masquerade as receipt acceptance.
6. Retained-size fixtures are stable when empty and monotonic as each collection grows.
7. sessions_map is zero for `None` and one for `Some(Session)`, counting current owned session state once.
8. Fold selection adds auxiliary bound plus incremental/full-rebuild equality cases; tool selection adds generation reuse/invalidation/mid-turn hydration cases; neither adds an explicit no-product-diff assertion/record.

## Harness-created run topology

The executor never precreates a run directory. For each eligible 900s and 3600s invocation independently:

1. Resolve the evidence root and snapshot immediate-child `run-*` directories by full path.
2. Create only a unique, absent temporary reporter file directly under that root; set NANO_MEM_STATS to it.
3. Run the harness with the evidence root, snapshot afterward, and require exactly one new immediate-child run directory.
4. Require the reporter destination absent. Hash/size the temp file, move it with `Move-Item -LiteralPath` without overwrite, then require destination hash/size equality.
5. Zero/multiple new directories, missing temp, existing destination, move/hash mismatch, or ambiguous cleanup fails. Cleanup may remove only the known temp file when safe.

The profile and eligible receipt paths are retained as two distinct exact resolved run directories. The neither/ineligible path retains only the profile run and its explicit Plan 04 no-op record.

## Profile classification and one rerun

The known failed attempt is not profile evidence: orchestration aborted at approximately 42 seconds after 8 turns, below the 25-turn reporter cadence. It is `aborted/unclassified`; missing reporter rows cannot select neither. Retain its audit artifacts for final exact-value canary coverage.

One clean rerun is permitted with budgets/harness unchanged. Before launch, PowerShell 5.1 durably writes a unique OS-temp audit JSON containing the exact release-binary path and true pre-run PID set. It starts node hidden, waits 1200000ms in try/finally, tree-kills and waits on timeout/error, performs the external exact-binary leak check, and always appends node PID, exit/error/timeout, taskkill exit/wait, post PIDs, and cleanup verdict. Verification parses that persisted record, requires post-minus-pre empty, requires kill exit0+wait for timeout/error, and independently requires current exact-binary PIDs be a subset of the recorded pre-set. Adjacent recomputed fake baselines are forbidden.

## Exact-value canary and cached-set equality

Execution blocks unless DEV-WP-0.2B signs ownership of only `scripts/canary/scan.mjs` for additive exact include-list/receipt and synthetic self-test modes. The extension is implemented and self-tested before profiling. Default scanner behavior remains unchanged. Final include-list mode internally reads the real key, never prints or writes its value, scans every exact listed file bytewise, and emits exact relative path/full SHA-256/bytes, key fingerprint, hits and verdict. Credential-shape pattern `wp02-credential-shapes-v1` may run as defense-in-depth but never substitutes for this exact-value scan.

After the final handoff is created and closed, the last task makes no later evidence-tree writes:

1. Independently recurse every file under all trusted exact aborted-attempt runs, the classified profile run when present, and optional eligible receipt run; add the exact handoff, normalize repo-relative, and reject duplicate/out-of-root paths.
2. Parse the supplied normalized inventory and require exact `Compare-Object` equality with the independently derived set before invoking the scanner.
3. Invoke the authorized include-list scanner. Missing/duplicate/out-of-root/unreadable file, scan error, nonzero hit, absent fingerprint, or non-PASS verdict fails.
4. Independently recompute every listed full SHA-256 and byte count and require exact equality with receipt rows and exact file-set equality.
5. Force-add each exact file individually; no wildcard or directory staging.
6. Enumerate ALL cached files and call `git check-ignore --no-index -- &lt;path&gt;` per path. Exit 0 means ignored, 1 means not ignored, and greater than 1 fails.
7. Reject every ignored cached path outside the exact approved run prefix(es) plus exact handoff. Require every scanned inventory file cached and exact equality between scanned inventory and the cached approved evidence set.
8. Require `.gitignore` unchanged. No unscanned evidence write follows final inventory creation.

Plan 05's `<automated>` gate itself reruns the include-list scanner, parses the true canary receipt, recomputes every hash/byte count, proves exact receipt/inventory equality and hits=0, verifies every inventory file is force-added, applies `git check-ignore --no-index` to every cached path, proves cached-approved/scanned equality, checks `.gitignore`, and confirms the builder branch/status. Descriptive handoff prose cannot satisfy this gate.

## Command failure contract

Native commands are sequenced as `command; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }`. PowerShell predicates inspect captured values directly rather than reading `$LASTEXITCODE` after cmdlets. No wildcard is passed to Node. A failed build, test, soak, parser, sample check, canary scan, or gate records the exact command, exit code, and relevant complete output and stops the affected claim.

## Phase closure matrix

| Terminal path | Plan 03 | Plan 04 | F-45 | Plan 05 reachable |
|---|---|---|---|---|
| fold selected | fold regression/correction record | eligible 1h run if tests green | FIXED only on B1 pass | yes |
| tool selected | tool regression/correction record | eligible 1h run if tests green | FIXED only on B1 pass | yes |
| neither selected | explicit no-op record and standard summary | explicit INELIGIBLE no-op record and standard summary | OPEN with measurements | yes |
| prerequisite/acceptance failure | failure record and summary | INELIGIBLE/FAILED record and summary | OPEN | yes, for blocked handoff |
| wrapper aborted/unclassified after allowed rerun | explicit blocked-classification/no-product record + normal summary | explicit blocked-classification/no-receipt record + normal summary | OPEN, no arm claim | yes, blocked audit handoff |

## Full gates

The final builder gate is the complete `just gate-all`, followed by the opt-in mem-stats focused test and combined-feature release build. One Critical/High audit and at most one fix round precede the final gate. The builder does not merge, push, create the integration worktree, self-promote, or claim CI green.
