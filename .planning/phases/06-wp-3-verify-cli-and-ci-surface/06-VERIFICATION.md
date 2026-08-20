---
phase: 06-wp-3-verify-cli-and-ci-surface
verified: 2026-08-21T17:10:00+07:00
status: passed
score: 7/7 must-haves verified
behavior_unverified: 0
overrides_applied: 0
---

# Phase 6: WP-3 Verify CLI and CI Surface Verification Report

**Phase Goal:** Users can run verified-change climbs, mint receipts, independently reverify them offline, and adopt a version-pinned required CI check.
**Verified:** 2026-08-21T17:10:00+07:00
**Status:** passed on the canonical builder tip; Landed remains pending integrator-owned no-ff merge, push, and exact-SHA six-leg CI.
**Re-verification:** No — initial goal-backward verification.

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|---|---|---|
| 1 | CLI-01: the binary exposes the closed mint, run-only, and receipt-check modes with the 0/1/2/3/6 contract. | VERIFIED | `main.rs` dispatches `verify` through `parse_args` and `run`; parser/unit coverage plus the independently run 14-test integration target exercised all three modes, invalid combinations, run-only 0/2/3, receipt 0/6, and no-receipt-on-red behavior. |
| 2 | CLI-02: canonical registry mappings and closure bodies drive climbs/materialization without duplicating nano-verify contracts. | VERIFIED | `nano-cli` imports the path dependency and `verify_cmd.rs` calls nano-verify registry, gate, climb, candidate-diff, expected-change, and receipt APIs. The exact `landed_contract_import_probe` passed; a 37-type declaration scan found no local contract redeclarations. The production registry is the intentional 41-byte exact empty schema-1 literal, while fixture registries are populated through nano-verify. |
| 3 | CLI-03: JSONL v1 is closed and identifier-only. | VERIFIED | `VerifyEvents` emits only the eight specified frame types with `v`, `session_id`, and monotonic `seq`; check frames contain only id/category/pass and climb frames only closed fields. `verify_authored_defect_red_identifiers_only` passed and explicitly rejects command/source/fixture leakage. |
| 4 | CLI-04: offline verification proves the receipt in a bounded detached fix-commit worktree, fails closed, and cleans up. | VERIFIED | Actual fixture binaries returned `valid`, `never-red`, `fabricated-commit`, `ancestry-unproven`, and `gate-mismatch` as specified. The passing target also exercised tamper/unknown-field/pin-drift/red-rerun paths. Unit tests cover real detached-worktree removal/registration pruning, timeout-before-gate, spawn/probe failure, and cleanup on every outcome. |
| 5 | CLI-05: the authoritative end-to-end and exit-matrix battery has behavioral teeth. | VERIFIED | Independent discovery found the exact 13 authoritative names once each plus one fixture helper; the full target passed 14/14 serially. The M01-M09 ledger has the exact unique operators/tests/commands, nonzero assertion REDs, zero GREENs, final product binding, and live restored blob equality. |
| 6 | CLI-06: operators have a schema/version-pinned, docs-only CI consumer; `.github` promotion is deferred. | VERIFIED | `VERIFY-CLI.md`, `CI-ADOPTION.md`, two docs-owned workflows, and the executable receipt-diff oracle exist and agree. The oracle independently proved A/M pass, D/R fail, unchanged pass; `actionlint` accepted both YAML files. Product diff has no `.github/**` changes, and both docs and request defer promotion until after WP-4 mutants. |
| 7 | PROV-02 and the WP-3 control boundary are exact. | VERIFIED | `UPSTREAM.md` records destination-specific receipt, event/gate, fixture, and docs/CI adaptations without verbatim-copy claims. Base-to-product diff stays inside the WP-3 ownership allowlist and does not touch `crates/nano-verify/**`, `.github/**`, or `docs/verify/gates.md`. |

**Score:** 7/7 truths verified (0 present-but-behavior-unverified).

### Required Artifacts

| Artifact | Expected | Status | Details |
|---|---|---|---|
| `crates/nano-cli/src/verify_cmd.rs` | Production parser, events, run-only, climb/materializer, receipt verifier | VERIFIED | 123,124 bytes; compiled, strict-Clippy clean, behaviorally exercised, and wired from `main.rs`. |
| `crates/nano-cli/tests/verify_cmd.rs` and fixtures | Exact 13-name hermetic battery with real Git histories | VERIFIED | Discovery found 14 total tests; all 14 passed, including the exact 13 contract names. |
| `gates/registry.json` | Exact empty schema-1 bootstrap | VERIFIED | Exact bytes are `{"gates":{},"requirements":{},"schema":1}` with length 41 and no production entries. The plan's prose count of 45 is arithmetically wrong; its authoritative literal and executable byte comparison are satisfied. |
| `docs/verify/**` | CLI contract and version-pinned docs-owned CI consumers | VERIFIED | Both workflows lint; A/M/D/R selector oracle passes; `.github/**` remains untouched. |
| `UPSTREAM.md` | WP-3 donor transformation ledger | VERIFIED | Four precise WP-3 rows cover code, tests/fixtures, and docs/CI. |
| `06-REVIEW.{md,json}` | One identity-bound Critical/High audit | VERIFIED | Product `40baef2718e2b305b9515273256be5673e4db4e6`, tree `13ef21e8...`, and canonical binary diff digest independently recompute; all four High findings are resolved and open Critical/High count is zero. |
| `06-MUTATION-RECEIPTS.json` | Exact M01-M09 mutation ledger | VERIFIED | Nine unique entries; every live product blob matches both pristine/restored hashes and every row binds the final product head. |
| `06-PROMOTION-REQUEST.json` | Request-only builder handoff | VERIFIED | Deny-unknown key set and literal six jobs are present; request names metadata parent `a7da73e...`; builder tip `7e22846...` has that sole parent and its only diff is the request file. |

### Key Link Verification

| From | To | Via | Status | Details |
|---|---|---|---|---|
| `main.rs` | `verify_cmd` | `parse_args` then async `run` | WIRED | Thin dispatch exists before version handling and returns the verifier exit code directly. |
| Registry requirement/gate | Gate runner and climb | nano-verify loader, closure, invocation, baseline/candidate runners | WIRED | Canonical imports compile; full-flow and run-only fixture tests pass. |
| Accepted candidate | Receipt | nano-verify parser/manifest, protected materializer, coherent Green rerun, store | WIRED | Full-flow mint test passes; red run preserves the pre-existing output and creates no store receipt. |
| Receipt | Offline verdict | preflight, detached worktree, pinned rerun, cleanup | WIRED | Roundtrip-valid and all fail-closed fixture classes pass; cleanup tests exercise real worktree removal. |
| Git receipt diff | CI verifier | D/R hard fail; A/M invoke pinned verifier | WIRED | Executable PowerShell oracle and actionlint both pass. |

### Data-Flow Trace (Level 4)

Not applicable to rendered UI. For the CLI data path, real fixture data flows from runtime-created Git A/B/C histories and populated canonical registries through binary execution, gate results, JSONL/verdict output, receipt persistence, and detached offline rerun. No hardcoded success path was observed.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|---|---|---|---|
| Exact-name discovery | `cargo test -p nano-cli --test verify_cmd -- --list` | Exact 13 names once each; 14 tests total | PASS |
| Full CLI fixture/exit battery | `cargo test -p nano-cli --test verify_cmd -- --test-threads=1` | 14 passed, 0 failed | PASS |
| Landed public contract | `cargo test -p nano-cli --lib verify_cmd::tests::landed_contract_import_probe -- --exact --nocapture` | 1 passed | PASS |
| CI receipt selector | `powershell ... docs/verify/ci/test-receipt-diff.ps1` | A/M pass; D/R fail; unchanged passes | PASS |
| Strict lint | `cargo clippy -p nano-cli --all-targets -- -D warnings` | Exit 0 | PASS |
| Workflow syntax | `actionlint docs/verify/ci/verify-receipt-check.yml docs/verify/ci/verify-dogfood.yml` | Exit 0 | PASS |

### Probe Execution

No `probe-*.sh` is declared for Phase 6. The phase-declared executable CI oracle was run independently and passed.

### Requirements Coverage

| Requirement | Source Plans | Status | Evidence |
|---|---|---|---|
| CLI-01 | 01, 02, 03, 06, 09, 10 | SATISFIED | Closed parser/dispatch and behavioral exit-matrix target. |
| CLI-02 | 01, 02, 04, 05, 06, 09, 10 | SATISFIED | Canonical nano-verify imports, registry-to-climb wiring, sealed materializer, no redeclarations. |
| CLI-03 | 01, 02, 04, 06, 09, 10 | SATISFIED | Closed JSONL/event vocabulary and identifiers-only regression. |
| CLI-04 | 02, 03, 06, 09, 10 | SATISFIED | Bounded detached offline rerun, canonical fail-closed verdicts, cleanup. |
| CLI-05 | 02-06, 09, 10 | SATISFIED | Exact 13-name battery plus mutation receipts and live blob binding. |
| CLI-06 | 07, 09, 10 | SATISFIED | Version-pinned docs-only consumers, validated selector, explicit deferred promotion. |
| PROV-02 | 08-10 | SATISFIED | Exact transformation rows and ownership inventory. |

No Phase 6 requirement is orphaned.

### Anti-Patterns Found

No unreferenced `TBD`, `FIXME`, or `XXX` marker, placeholder implementation, imported-contract redeclaration, forbidden ownership edit, or post-audit product drift was found in the Phase 6 product surfaces.

Disconfirmation checks also passed: the registry is intentionally empty rather than pretending production Gate Cards exist; the 13-name oracle rejects zero-test discovery and uses real Git fixture histories; timeout, cleanup, protected-path, rollback, red-rerun, and unknown-field paths all have direct tests. The remaining integration/push/CI work is explicitly outside the builder's authority and is not represented as Landed.

### Human Verification Required

None. The phase's user-visible behavior is CLI/protocol behavior with automated binary, fixture, mutation, workflow, and cleanup evidence.

### Promotion Status

Implemented and independently verified at product head `40baef2718e2b305b9515273256be5673e4db4e6`; request-only builder tip is `7e228466ad92d440ef0770b07cce0e5911368f91`. Integration no-ff merge, push, and exact-SHA six-leg CI remain pending and must complete before Phase 7 starts. This report does not claim Landed.

### Gaps Summary

No implementation gap blocks the WP-3 builder handoff. The only remaining work is the explicitly integrator-owned promotion gate.

---

_Verified: 2026-08-21T17:10:00+07:00_
_Verifier: the agent (ferrox-verifier)_

