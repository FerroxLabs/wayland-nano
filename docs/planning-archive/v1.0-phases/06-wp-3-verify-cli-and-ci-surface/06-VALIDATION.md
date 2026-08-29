---
phase: 6
name: WP-3 Verify CLI and CI Surface
status: planned
nyquist_compliant: true
validated: 2026-08-21
---

# Phase 6 Validation Strategy

## Test Infrastructure

| Property | Value |
|---|---|
| Framework | Rust test harness, PowerShell hermetic CI oracle, actionlint |
| Quick command | `cargo test -p nano-cli --test verify_cmd -- --test-threads=1` |
| Full command | `just gate-all` |
| Roots | Matching canonical `F:\Temp\Codex` TEMP/TMP and `F:\CargoTarget\wayland-nano-wp3` |

## Sampling Rate

- Every one of 18 tasks has runnable automated verification and a measurable done predicate.
- Run the focused filter/oracle after each task and the focused Phase 6 target after each wave.
- After workflow creation, also run actionlint and `docs/verify/ci/test-receipt-diff.ps1`.
- Plan 06-10 runs exact-name discovery, the full target, CI oracle, actionlint, strict Clippy, cargo-deny, and `just gate-all` on final reviewed bytes.
- No three consecutive tasks lack automated validation; intended focused feedback latency is at most 180 seconds.

## Exact Task Map

| Task | Plan/Wave | Requirements | Automated evidence | Status |
|---|---|---|---|---|
| 06-01-T1 | 06-01 / 1 | CLI-01/02/03 | parser + import probe + check | planned |
| 06-01-T2 | 06-01 / 1 | CLI-01/02 | 45-byte bootstrap + loader split | planned |
| 06-07-T1 | 06-07 / 1 | CLI-06 | asserted docs grep | planned |
| 06-07-T2 | 06-07 / 1 | CLI-06 | actionlint + A/M/D/R oracle + pin/scope | planned |
| 06-02-T1 | 06-02 / 2 | CLI-03/05 | event/leakage tests + Clippy | planned |
| 06-02-T2 | 06-02 / 2 | CLI-01/02/04/05 | run-only/deadline/receipt-entry tests | planned |
| 06-03-T1 | 06-03 / 3 | CLI-04/05 | locked-order preflight tests | planned |
| 06-03-T2 | 06-03 / 3 | CLI-04/05 | rerun/budget/cleanup tests + Clippy | planned |
| 06-04-T1 | 06-04 / 4 | CLI-02/05 | detached baseline tests | planned |
| 06-04-T2 | 06-04 / 4 | CLI-02/03/05 | Effects/climb/mint tests + Clippy | planned |
| 06-05-T1 | 06-05 / 5 | CLI-02/05 | confinement/protection/oracle tests | planned |
| 06-05-T2 | 06-05 / 5 | CLI-02/05 | apply/commit/rollback tests + Clippy | planned |
| 06-06-T1 | 06-06 / 6 | CLI-05 | runtime Git fixture tests | planned |
| 06-06-T2 | 06-06 / 6 | CLI-01..05 | exact 13 plus deny-unknown exact M01–M09 mutation ledger | planned |
| 06-08-T1 | 06-08 / 7 | PROV-02 | provenance + frozen ownership inventory | planned |
| 06-09-T1 | 06-09 / 8 | CLI-01..06/PROV-02 | closed product/diff identity + builder-distinct independent reviewer + metadata suffix | planned |
| 06-10-T1 | 06-10 / 9 | CLI-01..06/PROV-02 | identity/base/diff + exact13 + public-contract + mutation + ownership + canary + final gates | planned |
| 06-10-T2 | 06-10 / 9 | CLI-01..06/PROV-02 | deny-unknown literal request schema, exact review binding, six jobs, expectations, and sole dirty path | planned |

## Exact Named-Test Oracle

1. `verify_full_flow_green_mints_receipt`
2. `verify_authored_defect_red_identifiers_only`
3. `verify_receipt_roundtrip_valid`
4. `verify_receipt_tampered_fails_closed`
5. `verify_receipt_fabricated_commit`
6. `verify_receipt_unknown_field_fails_closed`
7. `verify_receipt_green_only_is_never_red`
8. `verify_receipt_gate_pin_drift`
9. `verify_exit_code_matrix`
10. `verify_red_run_writes_no_receipt`
11. `verify_receipt_ancestry_unproven`
12. `verify_receipt_rerun_red_is_gate_mismatch`
13. `verify_run_only_resolves_artifact_and_exit_codes`

Every name must be discovered exactly once and every individual exact invocation must report one passed test; zero-test output fails.

## Bootstrap / Wave 0 Handling

No standalone Wave 0 plan is needed. Dependency-ordered tasks create prerequisites before consumers:

- 06-01-T1 creates parser/run-with scaffolding; 06-01-T2 creates the exact registry bootstrap.
- 06-02-T1 creates clock/generation/gate/Git/filesystem/event seams before later orchestration.
- 06-06-T1 creates fixture content/helper before 06-06-T2 runs the battery.
- 06-07-T2 creates the runnable CI status oracle before 06-10-T1 consumes it.

These are planned RED scaffolds, not missing validation prerequisites.

## Manual Boundary

Product behavior is automated. Detached no-ff integration, push, and exact-SHA six-job existing CI are integrator-owned after the builder request and are not claimed here. `.github/workflows/**` promotion is not authorized in WP-3 and remains deferred until after WP-4 sealed mutants land.

## Sign-Off

- [x] All 18 tasks have automated verification and measurable done predicates.
- [x] CLI-01..06 and PROV-02 map to tasks and final evidence.
- [x] Exact discovery rejects zero-test false passes.
- [x] Receipt-check preserves parse → red → Git → registry → rerun; nonrepo/Git/temp/budget failures are Unverifiable/6.
- [x] The runnable CI oracle proves A/M pass and D/R fail using actual name-status output.
- [x] Audit metadata is a strict allowlisted suffix after committed product_head/product_tree; final gates prove ancestry and zero product drift.
- [x] Review JSON denies unknown fields, records builder/auditor/rechecker identities, permits auditor==rechecker for one reviewer role, and forbids either reviewer identity matching the builder.
- [x] Both audit closure and final gates independently recompute product base/head/tree and canonical binary-diff digest.
- [x] Final T1 explicitly runs public-contract/no-redeclaration, mutation-ledger, ownership, and exact include-list canary oracles fail-fast.
- [x] Final T2 validates literal schema/workflow/path/base/product/metadata/deferral/expectation values and the exact sorted six-job set.
- [x] `06-MUTATION-RECEIPTS.json` is explicit Phase 6 ownership; its exact M01–M09 set binds unique operators/tests/commands, selected_count=1, assertion RED, byte restore, GREEN, and final live product head/blob hashes.
- [x] WP-3 authorizes no `.github/workflows/**` promotion; the request encodes deferral until after WP-4 mutants.
- [x] Final sampling includes focused, workflow, dependency, strict-lint, and full-workspace gates.

**Approval:** validation design complete and ready for independent plan recheck; execution evidence remains pending.
