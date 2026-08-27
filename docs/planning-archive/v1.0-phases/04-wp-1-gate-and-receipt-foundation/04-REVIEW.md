---
phase: 04-wp-1-gate-and-receipt-foundation
reviewed: 2026-08-17T16:31:19Z
depth: deep
base_sha: db0b678dc13e9486f9328808854598a0c5ba8725
head_sha: 8b8ee719de267ed9a9aaa1d68aefb08e4f909e0c
files_reviewed: 11
files_reviewed_list:
  - Cargo.toml
  - Cargo.lock
  - crates/nano-verify/Cargo.toml
  - crates/nano-verify/src/lib.rs
  - crates/nano-verify/src/error.rs
  - crates/nano-verify/src/registry.rs
  - crates/nano-verify/src/gate.rs
  - crates/nano-verify/src/receipt.rs
  - crates/nano-verify/tests/gate_contract.rs
  - crates/nano-verify/tests/receipt_git.rs
  - UPSTREAM.md
findings:
  critical: 0
  high: 2
  warning: 0
  info: 0
  total: 2
status: issues_found
---

# Phase 04: WP-1 Gate and Receipt Foundation Code Review

**Review marker:** `WP1-BINDING-AUDIT-COMPLETE`
**Disposition:** BLOCKED — two High findings remain open.

## Summary

Deep review of the complete binding WP-1 diff found two High correctness/security defects. The gate subprocess path cannot produce a Green or Red outcome for any nonempty real Gate Card, and receipt preflight can be redirected to an attacker-selected Git object database through inherited environment variables. No Critical finding was found. Medium/Info observations are intentionally omitted.

Focused evidence: `$env:TEMP='F:\\Temp\\Codex'; $env:TMP='F:\\Temp\\Codex'; $env:CARGO_TARGET_DIR='F:\\CargoTarget\\wayland-nano'; cargo test -p nano-verify` passed 31 tests, but the gate contract tests encode the first defect as their expected result and no receipt test exercises hostile Git object-environment variables.

## Narrative Findings (AI reviewer)

### HIGH-01: Production gate execution always discards the Gate Card inventory

- **Severity:** High; **classification:** BLOCKER (correctness; release blocking)
- **Status:** OPEN
- **File/symbol/line evidence:** `crates/nano-verify/src/gate.rs:18-124`, `run_gate`; specifically line 124 calls `parse_gate_output(..., &[])`. `parse_gate_output` rejects an empty inventory at lines 347-353. `crates/nano-verify/tests/gate_contract.rs:115-167` then expects `InconsistentSummary` for valid `gate: 1/1` subprocess output, so the tests institutionalize rather than detect the broken contract.
- **Impact:** Every successfully spawned gate with a real nonempty inventory is forced into `FailClosed(InconsistentSummary)`. A valid green gate can never become `GateOutcome::Green`, and valid known failures can never become `GateOutcome::Red`; WP-2/WP-3 therefore cannot use this WP-1 execution foundation.
- **Concrete reproducer:** Run `cargo test -p nano-verify run_gate_parses_stdout_despite_nonzero_exit -- --exact --nocapture`. The fixture prints `gate: 1/1`, but the test passes only because it expects `FailClosed(InconsistentSummary { passed: 1, total: 1 })`. The same result is asserted by `run_gate_artifact_path_is_final_argv` and `run_gate_env_baseline_allowlist`. Directly compare with `parse_gate_output("gate: 1/1", &[("TG-01".into(), FailCategory::Value)])`, which returns Green: the subprocess path has thrown away the sole authority needed to reach that result.
- **Required fix:** Make the authoritative inventory available to `run_gate` without leaking Gate Card contents to model-facing APIs (for example, add it to the invocation as a verifier-private/opaque field or pass it as an explicit execution-only argument), and pass that inventory to `parse_gate_output`. Change the real-process tests to require Green/Red full-inventory verdicts for valid stdout, including the nonzero-exit case; retain explicit empty-inventory fail-closure coverage separately.

### HIGH-02: Receipt Git probes inherit object-database override variables and can validate commits from another repository

- **Severity:** High; **classification:** BLOCKER (security/correctness; receipt authenticity bypass)
- **Status:** OPEN
- **File/symbol/line evidence:** `crates/nano-verify/src/receipt.rs:162-177`, `git_probe_with_absence`, constructs `Command::new("git")` without `env_clear` and removes only five variables. It leaves Git object-routing variables such as `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`, and `GIT_COMMON_DIR` inherited. The authenticity decisions at lines 96-145 trust those probes to establish commit existence, ancestry, and test existence. `crates/nano-verify/tests/receipt_git.rs:143-286` contains no hostile-environment case.
- **Impact:** A caller-controlled ambient environment can make `preflight_receipt` return `ReceiptPreflight::Ready` for observed/fix commits that are absent from the repository identified by `repo_root`, violating the genuine-red and repository-confinement guarantee. The registry pin does not repair the false commit/test/ancestry proof.
- **Concrete reproducer:** Create repositories A and B. In B, create an observed commit containing `tests/red.rs` and a descendant fix commit; build a schema-1 receipt from those two B SHAs while passing A as `repo_root` and a matching in-memory registry. Set `GIT_OBJECT_DIRECTORY=<B>/.git/objects` (or `GIT_ALTERNATE_OBJECT_DIRECTORIES=<B>/.git/objects`) before calling `preflight_receipt`. `rev-parse --is-inside-work-tree` still succeeds for A, while `cat-file`, `merge-base --is-ancestor`, and `cat-file <observed>:tests/red.rs` resolve through B's inherited object store, allowing the preflight to reach `Ready` even though neither commit belongs to A.
- **Required fix:** Launch probes with `env_clear`, restore only the minimal executable-resolution/platform baseline needed to start Git, and set the intended Git controls explicitly. At minimum remove every repository/object/config override (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_INDEX_FILE`, `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_*`/`GIT_CONFIG_VALUE_*`, plus hook/prompt/SSH channels), or use a centralized proven scrub helper. Add a serialized hostile-environment integration test demonstrating that foreign object databases yield `FabricatedCommit`/`Unverifiable`, never `Ready`.

## Finding Ledger

| ID | Severity | Boundary | Status | Required disposition |
|---|---|---|---|---|
| HIGH-01 | High | Gate execution / parser inventory | OPEN | Wire authoritative inventory into real execution and require Green/Red in subprocess tests. |
| HIGH-02 | High | Receipt authenticity / scrubbed Git probes | OPEN | Clear hostile Git environment and add a foreign-object-database regression test. |

---

_Reviewed: 2026-08-17T16:31:19Z_
_Reviewer: ferrox-code-reviewer_
_Depth: deep_
