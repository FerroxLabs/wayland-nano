# B-ENV-02 — Track A baseline failures vs. the ported stack

Track A recorded 4 failures on the **unchanged donor baseline** (nextest run
`c59e02e0-262a-4534-a9a5-83f0d1e325a5`, their `BASELINE.md`). This doc classifies
each against Track B's ported implementation. Executables where possible;
provisioning-gated items marked.

| # | Track A baseline failure | Track B status | Evidence |
|---|---|---|---|
| 1 | cancellation did not finish before test timeout | **PORTED & PASSING** | `job::nano_tests` (tree kill 3–4ms), `stdio_bridge` Ctrl+C → terminate → exit; `nanok3-tree-kill-probe` binary |
| 2 | elevated non-TTY spawn: `CreateProcessWithLogonW failed: 2`, timeout | **provisioning-gated; likely donor environment issue** | error 2 = ERROR_FILE_NOT_FOUND — consistent with `codex-command-runner.exe` not materialized in their baseline run; Track B helper discovery is unit-tested (`helper_materialization` 11 tests incl. resource-dir preference). Full proof after provisioning. |
| 3 | legacy capture timed out twice | **REPRO PASSED** — `capture.rs` end-to-end | 3 tests: echo+exit-0-no-timeout, outside-root write denied, cancellation fires at 1.5s into infinite child (elapsed dominated by ~30s spawn prep — token+ACL — not the wait; Track A's failure was a hang, ours terminates) |
| 4 | legacy TTY descendant did not start | **deliberately deferred (D8)** | ConPTY web not ported; `tty=true` fails closed typed (`ConPtyDeferred`, `HelperConptyDeferred`). Their donor failure is in the ConPTY path we intentionally do not replicate. |

## Repro notes

- (1) Their cancellation timeout smells like the same class as my first-run
  harness discovery: raw parent-kill on Windows orphans children — Job
  terminate is the answer, and it is measured at 3–4ms on this host.
- (3) RESULT: ported capture passes all three shapes. Classification of
  Track A's failure: donor-context (helper provisioning/codex-home state),
  not an inherited defect in the capture logic itself.
- (1) RESULT: cancellation terminates; caveat learned — end-to-end spawn
  prep (restricted token + session ACL rules) costs ~30s on this host per
  spawn, so latency assertions must measure from spawn-complete, not from
  test start. Noted for the C1.2 timing budget.
- (2) is the only one that genuinely needs the elevated provisioning to
  reproduce either way.
