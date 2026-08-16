---
wp: WP-0.2
builder_branch: feat/wp-0.2
builder_tip: bd56f8c
baseline: 566e3ac
outcome: measured_neither
f45_status: open
receipt_3600s: not_run_ineligible
---

# WP-0.2 builder handoff

## Builder boundary

This is a builder-only handoff from `.tmp-wt-vc-wp-0.2` on `feat/wp-0.2`. No merge, push, CI invocation, promotion, or integration-worktree action was performed or claimed. The integrator must independently review the staged evidence and product diff.

## Outcome

- Implemented: opt-in exact-schema memory reporter, Windows private-working-set sampling, recursive MCP retained accounting, governed exact-list scanner, tests, and retained profile evidence.
- Reachable: reporter-enabled fake-model profile and exact-value canary closure are locally runnable and passed their focused gates.
- Live-proven: a 901,636 ms fake-model profile completed with 57 reporter rows and 15 aligned PWS samples across three PIDs. This is not a live provider or shipped-binary receipt.
- Decision: measured `neither`; eligible fold auxiliaries were 28.094% of positive accounted growth and MCP retained growth was 0%.
- F-45: OPEN. No product correction landed and the 3,600-second B1 receipt was ineligible/not run. B1/B11 acceptance is not claimed.

## Exact retained roots

- Failed attempt: `scripts/soak/evidence/run-20260816T161856444Z` (`aborted_unclassified`, no arm).
- Classified profile: `scripts/soak/evidence/run-20260816T163631293Z` (`profile_state: classified`, `selected_arm: neither`).
- Receipt run: none.
- Handoff: `scripts/soak/evidence/WP-0.2-HANDOFF.md`.

## Critical/High audit

Audit scope was the complete WP diff from `566e3ac`, including correctness, security, protocol contamination, selected-arm exclusivity, ownership, evidence integrity, default scanner behavior, key disclosure, and test weakening.

| Severity | Finding | Disposition |
|---|---|---|
| High | Exact-list inventory paths are worktree-relative, but the scanner used its historical `../../../` parent root, causing linked-worktree inventory mismatch. | Fixed in the single bounded round: exact-list mode uses current worktree root; legacy default coverage retains the historical root. Synthetic self-test and focused suites pass. |
| High | Exact-list self-tests did not explicitly prove missing-file, lexical traversal, and realpath junction/symlink escape rejection. | Fixed in the authorized bounded final review round with fail-closed synthetic fixtures. |
| High | Promotion instructions incorrectly required the cached evidence diff to equal the full inventory, including unchanged tracked files. | Fixed in the authorized bounded final review round with the complete-index, stage-0 equality, and changed-subset invariant below. |

No Critical findings and no unresolved High findings remain. The initial bounded scanner-root fix and the authorized bounded final review-fix round are complete. No secret path or value was printed or persisted.

## Gates

- `cargo test -p nano-cli acp_mode::tests::mem_stats --features mem-stats`: PASS, 5 tests.
- `cargo test -p nano-cli incremental_fold_matches_full_rebuild`: PASS, 2 tests.
- `cargo test -p nano-agent`: PASS, 306 unit tests plus integration/doc tests.
- `node --check scripts/canary/scan.mjs`: PASS.
- `node scripts/canary/scan.mjs --self-test-include-list`: PASS.
- `just gate-all`: PASS on the unchanged rerun with a sufficient wrapper boundary; includes formatting, workspace clippy/tests, error-table check, and `gate-gen-check` contract check.
- `cargo build --release -p nano-cli -F nano-agent/soak-fake-model -F nano-cli/mem-stats`: PASS.

The first `just gate-all` invocation was externally terminated at a 120-second tool boundary during workspace tests and produced a Windows broken pipe. The exact unchanged command passed when rerun with a sufficient boundary; this was not a code fix round.

## Evidence closure instructions

After this handoff is finalized, the builder independently enumerates both exact run roots plus this file, freezes the normalized sorted inventory, performs the governed exact-value scan, and verifies exact list/filesystem/receipt hash-and-byte equality. The integrator should repeat those equalities, verify `.gitignore` is unchanged, require the complete indexed approved set to equal the scanned inventory, require every stage-0 blob hash and byte count to equal the scanned worktree file, and require the cached approved diff to equal only the changed approved inventory subset before promotion. Unchanged already-tracked inventory need not appear in the cached diff.

## Integrator checklist

1. Review the product/scanner diff from `566e3ac` and the measured-neither decision.
2. Re-run final inventory and receipt checks; prove complete indexed-approved equality, per-file stage-0/worktree hash-and-byte equality, changed/cached approved-subset equality, and unchanged ignore policy.
3. Re-run required gates in the integration environment.
4. Preserve F-45 as OPEN and do not claim a 3,600-second B1 receipt.
5. Merge/push/CI only under integrator authority.
