---
phase: 06-wp-3-verify-cli-and-ci-surface
plan: 05
subsystem: cli
tags: [rust, git, materializer, rollback, sealed-manifest]
requires:
  - phase: 06-wp-3-verify-cli-and-ci-surface
    provides: verified CandidateArtifact handoff and detached baseline identity
provides:
  - Trusted component-confined candidate materialization
  - Sealed-manifest staged and committed state verification
  - Proven precommit rollback and auditable postcommit retention
affects: [06-06, CLI-02, CLI-05]
tech-stack:
  added: []
  patterns: [single shared parser, sealed expected-state oracle, transactional git apply]
key-files:
  created: [.planning/phases/06-wp-3-verify-cli-and-ci-surface/06-05-SUMMARY.md]
  modified: [crates/nano-cli/src/verify_cmd.rs]
key-decisions:
  - "Use CandidateDiff only for ordered paths and ExpectedChangeManifest only for A/M/D and postimage expectations."
  - "Retain a coherent fix commit after postcommit failure; receipt minting stays blocked until Plan 06-06 reruns Green."
requirements-completed: [CLI-02, CLI-05]
duration: 32min
completed: 2026-08-21
status: complete
---

# Phase 6 Plan 5: Trusted Candidate Materializer Summary

**Accepted candidate bytes now cross a deterministic, component-confined Git transaction that verifies staged and committed state solely against nano-verify's sealed manifest and restores exact start state on every precommit failure.**

## Accomplishments

- Bound exact accepted bytes to one `parse_candidate_diff` call, one `derive_expected_changes` call, the artifact/diff SHA-256, retained base-tree digest, and canonical sorted changed-path digest.
- Added component-aware target confinement and symmetric protected-path overlap checks, including `.git` rejection and protected-target refusal.
- Added stdin-only `git apply --check --index --whitespace=error-all -` followed by one identical indexed apply, complete staged A/M/D and blob verification, sole-parent commit verification, and clean-state guards.
- Added mandatory precommit rollback to the captured start commit and left coherent postcommit state auditable without minting a receipt before the downstream pinned Green rerun.

## Task Commits

1. **RED: materializer confinement and transaction contracts** - `5abe2bf`
2. **GREEN: sealed candidate materialization** - `8a6b911`

## Verification

- `cargo test -p nano-cli verify_cmd::tests::materializer_confinement --lib -- --nocapture` - 3 passed.
- `cargo test -p nano-cli verify_cmd::tests::materializer_transaction --lib -- --nocapture` - 2 passed.
- `cargo clippy -p nano-cli --all-targets -- -D warnings` - passed.
- `just gate-all` with F:-only TEMP/TMP/CARGO_TARGET_DIR - passed, including 190 nano-cli unit tests (1 ignored live-gated test) and the full workspace suite.

## Deviations from Plan

None - implementation stayed within `verify_cmd.rs`; downstream rerun, receipt storage, and output-copy work remains owned by Plan 06-06.

## Known Stubs

None in the Plan 06-05 materializer. The deliberate exit-3 boundary after a coherent commit prevents receipt minting until Plan 06-06 supplies the required pinned Green rerun.

## Threat Flags

| Flag | File | Description |
|---|---|---|
| threat_flag: repository-mutation | `crates/nano-cli/src/verify_cmd.rs` | Candidate bytes may mutate the selected target only after shared parsing, sealed derivation, component confinement, and exact clean-start proof. |
| threat_flag: git-transaction | `crates/nano-cli/src/verify_cmd.rs` | Indexed apply and commit are verified against the sealed manifest; precommit failures roll back and postcommit history is retained. |

## Self-Check: PASSED

- Product file and summary exist.
- RED and GREEN commits exist on `worktree-agent-wp3-05`.
- Focused tests, strict Clippy, and `just gate-all` passed on final product bytes.
- Only the assigned Plan 06-05 product and summary paths changed.
