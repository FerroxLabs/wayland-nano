---
phase: 06-wp-3-verify-cli-and-ci-surface
plan: 06
subsystem: cli
tags: [verify, receipts, fixtures, mutation-testing]
requires: [06-05]
provides: [authoritative-verify-battery, pinned-green-receipt-minting, closed-mutation-ledger]
affects: [06-08, 06-09, 06-10]
tech-stack:
  added: []
  patterns: [hermetic-real-git-fixtures, canonical-registry-digest, byte-restored-mutation-receipts]
key-files:
  created: [crates/nano-cli/tests/verify_cmd.rs, crates/nano-cli/tests/fixtures/verify, .planning/phases/06-wp-3-verify-cli-and-ci-surface/06-MUTATION-RECEIPTS.json]
  modified: [crates/nano-cli/src/verify_cmd.rs]
key-decisions:
  - "Use platform-native fixture interpreters while preserving one canonical registry and receipt authority."
  - "Mint only after the retained baseline evidence, materialized fix commit, and pinned Green rerun all agree."
  - "Canonicalize both confinement operands before starts_with comparisons and de-verbatimize Git worktree arguments on Windows."
requirements-completed: [CLI-01, CLI-02, CLI-03, CLI-04, CLI-05]
duration: 95min
completed: 2026-08-21
status: complete
---

# Phase 6 Plan 6: Authoritative Verify Battery Summary

Hermetic real-Git fixtures now prove all 13 exact WP-3 CLI behaviors offline, while the production flow preserves baseline evidence through a materialized fix, requires a pinned Green rerun, and atomically stores and copies the resulting canonical receipt.

## Accomplishments

- Added content-only broken/fixed sources, gate/card authorities, and eight receipt templates resolved from actual A/B/C commits and `nano_verify::closure_digest`.
- Added the exact 13 authoritative tests plus a fixture bootstrap test under serialized F:-only roots.
- Completed the post-materialization rerun and receipt boundary without changing `nano-verify`.
- Executed M01-M09 serially with unique operators, assertion-specific RED, identical-command GREEN, and exact blob restoration.
- Passed `just gate-all`, including fmt, workspace Clippy with `-D warnings`, the full workspace suite, and generated-contract checks.

## Task Commits

1. `f21d62f` — authoritative fixture and exact-name test battery.
2. `d3b0120` — pinned Green rerun and receipt mint/store/copy boundary.
3. `2dba3ac` — closed M01-M09 mutation ledger.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical functionality] Completed receipt minting boundary**
- **Found during:** Task 2 RED execution.
- **Issue:** Plan 06-05 deliberately stopped with exit 3 after a coherent materialized commit.
- **Fix:** Retained eligible baseline evidence, reran the selected pinned gate at the fix commit, required Green and clean identity, then minted and atomically stored/copied the receipt.
- **Files modified:** `crates/nano-cli/src/verify_cmd.rs`
- **Commit:** `d3b0120`

**2. [Rule 1 - Windows path bug] Canonical/non-verbatim path mismatch**
- **Found during:** Detached fixture and receipt reruns.
- **Issue:** Confinement compared a canonical candidate against a lexical root, while Git received verbatim `\\?\` worktree paths.
- **Fix:** Canonicalized both confinement operands and converted validated TEMP-derived Git worktree arguments to native Windows form.
- **Files modified:** `crates/nano-cli/src/verify_cmd.rs`
- **Commit:** `d3b0120`

## Known Stubs

None.

## Threat Flags

| Flag | File | Description |
|---|---|---|
| threat_flag: filesystem-trust-boundary | `crates/nano-cli/src/verify_cmd.rs` | Canonical confinement and detached worktree lifecycle protect receipt and materializer paths. |
| threat_flag: receipt-attestation | `crates/nano-cli/src/verify_cmd.rs` | Receipt bytes are emitted only after pinned Green verification and canonical store persistence. |

## Self-Check: PASSED

- All created files and commits exist.
- Exact-name discovery found each of 13 required names once; every exact command passed.
- Ledger contains exactly M01-M09 and final live blob hashes equal pristine/restored hashes.
- `just gate-all` passed on final committed product/test bytes.
