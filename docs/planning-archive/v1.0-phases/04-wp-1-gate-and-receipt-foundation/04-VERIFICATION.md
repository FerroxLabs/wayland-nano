---
phase: 04-wp-1-gate-and-receipt-foundation
verified: 2026-08-20T05:45:00Z
status: passed
score: 13/13 must-haves verified
behavior_unverified: 0
overrides_applied: 0
gaps: []
---

# Phase 4: WP-1 Gate and Receipt Foundation Verification Report

**Phase Goal:** Users and tools can run deterministic fail-closed gates and persist independently preflightable red-green evidence using the canonical shared interfaces.
**Verified:** 2026-08-20T05:45:00Z
**Status:** passed
**Re-verification:** No — initial goal-backward verification
**Verified branch:** `feat/wp-1` at `abf0c4a1d9a07e6056aa2ba3a7c96682b9790b59`
**Recorded base:** `db0b678dc13e9486f9328808854598a0c5ba8725`

## Goal Achievement

The implementation was inspected directly; SUMMARY claims were not accepted as implementation evidence. The verifier independently ran the complete crate target and supply-chain check, enumerated the authoritative WP-1 test names, rechecked ownership and forbidden-surface oracles, inspected both audit fixes, and validated the two external canary receipts against the current bytes without reading or reporting the governed key or its fingerprint.

### Observable Truths

| # | Truth | Status | Evidence |
|---:|---|---|---|
| 1 | A caller can execute an argv-only gate with canonical scrubbed environment, bounded output, timeout/tree termination, and exit-code-independent verdicts. | VERIFIED | `run_gate` uses `Command::new` with argv, appends the artifact last, calls `env_clear`, restores only IFACE baseline names plus declared env, retains at most 16 MiB, bounds execution with `tokio::time::timeout`, and uses Windows Job Objects or a Unix process group. Independent `cargo test -p nano-verify` passed the real-process tests, including Green/Red on nonzero exit and descendant death after timeout. |
| 2 | Missing/empty output, empty inventory, unknown IDs, inconsistent totals, spawn failure, timeout, and over-cap output fail closed. | VERIFIED | `parse_gate_output` has no default-Green path and checks nonempty inventory plus `m == inventory.len()` and `n == m - failures`; `run_gate` maps operational faults to bounded fail-closed outcomes. Parser and runner edge tests passed, including `run_gate_empty_inventory_fails_closed` and the bounded-output arm inside `run_gate_timeout_fails_closed`. |
| 3 | Gate parsing uses the last valid summary and reconstructs the full authoritative inventory. | VERIFIED | Parser lines 314-372 rebuild every `CheckVerdict` from the caller-provided inventory and flip only declared FAIL IDs. All nine authoritative parser tests passed independently. |
| 4 | Gate projections expose canonical scores/failure keys without leaking source, commands, expected values, fixtures, or ambient secrets. | VERIFIED | `score()` derives only pass/total counts; `fails()` returns only canonical `ID category` values or fixed sentinels. `GateOutcome` carries no source/command fields, and the environment test proved an ambient sentinel was absent. |
| 5 | A schema-1 standalone receipt can be minted with canonical red evidence and no final verification claim. | VERIFIED | `Receipt`/`FailingRun` are `deny_unknown_fields`; `canonical_receipt` emits normalized no-newline JSON; `mint_receipt` rejects zero exit and malformed evidence. `ReceiptPreflight` has `Ready` but no `Valid` arm, and no production function constructs `VerifyVerdict::Valid`. |
| 6 | Receipt writes use bounded exclusive locking and true platform atomic replacement; readers retry once then fail closed. | VERIFIED | `acquire_lock` uses `create_new`, 50 ms retry/10 s production deadline and optional >60 s stale break. Writes use a same-directory `NamedTempFile`, `sync_all`, `MoveFileExW(...REPLACE_EXISTING)` on Windows or rename+directory fsync on Unix, with no remove-before-replace path. All three authoritative store tests passed. |
| 7 | Receipt preflight proves schema/red evidence, both commits, ancestry, test existence, requirement mapping, and registry pin before returning `Ready`. | VERIFIED | `preflight_receipt` performs the checks in fail-closed order and returns typed non-Ready outcomes. All nine materialized Git-fixture tests passed. |
| 8 | Git receipt probes cannot be redirected through an ambient object database. | VERIFIED | Audit fix `eb97974` added `env_clear` with a minimal launch environment and fixed noninteractive/config controls. The independently rerun `hostile_object_database_cannot_supply_foreign_commits` test passed; foreign commits cannot reach `Ready`. |
| 9 | Registry loading is schema-1, canonical/NFC, pin-checked, requirement-complete, and repo-confined. | VERIFIED | `load_registry`, `closure_digest`, `check_inventory`, and `confined_existing` substantively implement the IFACE registry contract. The three registry tests passed, including pinned digest, unknown-field, drift, dangling-map, missing/escaping path, and script-shape cases. |
| 10 | The public crate surface hands WP-1 primitives downstream without implementing WP-2/WP-3/WP-4. | VERIFIED | `lib.rs` exports registry, gate, receipt, store, and preflight primitives only. Base-to-HEAD inspection found no `climb.rs`, `engine.rs`, `crates/nano-cli/**`, `gates/**`, or `.github/**` changes. |
| 11 | The complete named WP-1 battery and full crate target pass after audit closure. | VERIFIED | Independent enumeration found all 29 WP-1 names exactly once, zero missing/duplicates, and zero WP-2 names. Independent full crate execution passed 33/33 tests: 17 unit, 7 gate-contract, 9 receipt-git; doc tests passed. |
| 12 | Dependency, provenance, ownership, full-local-gate, and canary evidence meet the WP-1 handoff contract. | VERIFIED | Independent `cargo deny check` returned advisories/bans/licenses/sources OK. Lock diff adds only `nano-verify` and exact `unicode-normalization 0.1.24`; target `windows-sys 0.52` features are the four authorized features. Product diff is exactly 11 authorized paths; planning changes stay in the exact allowlist. Primary receipt is current-hash exact for 20/20 files, 306,786 bytes, 0 hits, receipt SHA-256 `302f049e8d65bb575de41cd95d25e478316af8f3bd603a9f41054a0805636895`; supplemental summary receipt is current-hash exact for 1/1 file, 7,253 bytes, 0 hits, receipt SHA-256 `f8e595d4b68587c12e3cf174528b061b0872b19d45e7aa042dccde8e7782c2e8`. The retained full `just gate-all` handoff records exit 0 at this same product byte set; both exact receipts bind those product bytes. |
| 13 | Exactly one Critical/High audit and at most one bounded fix round closed all release-blocking findings without scope expansion. | VERIFIED | The binding review at `8b8ee71` found Critical 0/High 2. The single fix commit `eb97974` changed only `gate.rs`, `receipt.rs`, and `gate_contract.rs`; direct diff inspection confirms authoritative inventory now reaches the parser and Git probes clear ambient routing. Their focused regressions and full crate target pass at final HEAD. No second fix commit or open Critical/High item exists. |

**Score:** 13/13 truths verified (0 present-but-behavior-unverified)

## Required Artifacts

| Artifact | Exists/substantive | Wiring | Status | Details |
|---|---|---|---|---|
| `crates/nano-verify/Cargo.toml` | Yes | Root workspace member | VERIFIED | Bottom-of-graph dependency set; no internal `nano-*`, network, regex, or git2 dependency. |
| `src/error.rs` | Yes | Re-exported by `lib.rs`; consumed by registry/receipt | VERIFIED | Closed crate-local infrastructure taxonomy; no `NanoErrorKind` change. |
| `src/registry.rs` | Yes | Re-exported and consumed by receipt preflight | VERIFIED | Canonical digest, envelope validation, mapping, containment, script-shape, and inventory logic are implemented and tested. |
| `src/gate.rs` | Yes | Re-exported; real subprocess tests call production runner/parser | VERIFIED | Pure parser and contained runner are substantive; inventory wiring closes the original audit defect. |
| `src/receipt.rs` | Yes | Re-exported; consumes registry and system Git; integration tests invoke preflight | VERIFIED | Canonical document, minting, atomic store, reader, and preflight are implemented. |
| `tests/gate_contract.rs` | Yes | Cargo integration-test target | VERIFIED | Seven tests at final HEAD; the five authoritative runner names plus fixture and empty-inventory regression. |
| `tests/receipt_git.rs` | Yes | Cargo integration-test target | VERIFIED | Nine materialized-repository preflight tests. |
| `UPSTREAM.md` | Yes | Exact destination rows for every WP-1 adaptation | VERIFIED | Registry is contract-defined; gate and receipt donor transformations and rejected donor behavior are recorded. WP-2 rows are correctly absent until WP-2. |
| `04-09-SUMMARY.md` | Yes | Supplemental canary receipt binds its final bytes | VERIFIED | Builder-only handoff is explicit about no merge/push/integration/CI/self-promotion. |

## Key Link Verification

| From | To | Via | Status | Details |
|---|---|---|---|---|
| Registry Gate Card inventory | Real gate process result | Explicit `run_gate(..., inventory)` then `parse_gate_output` | WIRED | Full Green/Red inventory behavior independently passed. |
| Closure body | Registry pin | NFC canonical JSON then SHA-256 | WIRED | Pinned canonical vector and drift rejection passed. |
| Receipt bytes | Repository history | Env-cleared, bounded system-Git probes | WIRED | Commit peel, ancestry, and test-object checks run before registry pin and `Ready`. |
| Receipt object | Durable receipt path | Lock → same-directory tempfile → sync → platform atomic replace | WIRED | No target removal or non-atomic fallback exists. |
| `lib.rs` | WP-1 modules | Explicit public re-exports | WIRED | Downstream API is reachable while later-WP modules remain absent. |
| Adapted source | Provenance authority | `UPSTREAM.md` destination/donor/transformation rows | WIRED | All three WP-1 adapted/contract-defined source files have exact rows. |

## Data-Flow Trace (Level 4)

Not applicable: Phase 4 produces a Rust library, not a dynamic rendering surface. The relevant runtime flows are exercised by the subprocess, filesystem, and materialized-Git behavioral tests above.

## Behavioral Spot-Checks

| Behavior | Command/method | Result | Status |
|---|---|---|---|
| Full WP-1 crate behavior | `cargo test -p nano-verify` with TEMP/TMP/CARGO_TARGET_DIR on F: | 33 passed, 0 failed | PASS |
| Exact WP-1 inventory | `cargo test -p nano-verify -- --list` + exact 29-name comparison | 29/29 once; 0 missing/duplicate; WP-2 0 | PASS |
| Dependency policy | `cargo deny check` | advisories, bans, licenses, sources OK | PASS |
| Ownership boundary | exact base-to-HEAD set comparison | 11/11 exact; no forbidden surface | PASS |
| Canary evidence | safe-field receipt parse + current file SHA/byte recomputation | primary 20/20 and supplemental 1/1 exact; zero hits | PASS |

All commands ran with `TEMP=F:\Temp\Codex`, `TMP=F:\Temp\Codex`, and `CARGO_TARGET_DIR=F:\CargoTarget\wayland-nano`.

## Probe Execution

No conventional or phase-declared shell probe exists for WP-1. Verification used the authoritative Rust test targets and exact external canary receipts.

## Requirements Coverage

| Requirement | Status | Evidence |
|---|---|---|
| GATE-01 | SATISFIED | Production runner is argv-only, environment-cleared, output/time bounded, and tree-contained; real-process tests pass. |
| GATE-02 | SATISFIED | Nine parser and runner fault tests prove last-summary/full-inventory/exit-independent/fail-closed behavior. |
| GATE-03 | SATISFIED | Public projections contain only counts, canonical IDs/categories, or fixed sentinels; ambient sentinel is scrubbed. |
| RCPT-01 | SATISFIED | Strict standalone schema-1 red receipt and canonical serializer/mint validation exist and pass. |
| RCPT-02 | SATISFIED | Canonical lock/tempfile/fsync/platform-replace/retry algorithm exists; all store tests pass. |
| RCPT-03 | SATISFIED | Preflight proves red/Git/test/mapping/pin evidence and can return only `Ready`, reserving final `Valid` for WP-3. |
| RCPT-04 | SATISFIED | All 29 named WP-1 tests exist exactly once and pass as part of the independently rerun 33-test crate target. |
| PROV-01 | SATISFIED | Exact source-specific rows cover registry, gate, and receipt; no WP-2 provenance is preclaimed. |

No Phase-4 requirement is orphaned: all eight IDs appear in the Phase-4 plans and ROADMAP mapping.

## Scope and Promotion Controls

- Product delta: exactly the authorized 11 paths.
- Planning delta: only the Phase-04 directory and the four named Ferrox bookkeeping files.
- Nested `.git` directories under `crates/nano-verify`: zero.
- Generated error/contract artifact delta: zero.
- WP-2 climb/engine, WP-3 CLI/final rerun, WP-4 cards, `.github`, WP-5, and WP-6 surfaces: absent.
- Branch remains builder-owned: no WP branch push, detached integration merge, master push, CI-green claim, or self-promotion is represented by this phase verification.

## Anti-Patterns and Disconfirmation Pass

No `TBD`, `FIXME`, `XXX`, `TODO`, `HACK`, placeholder, or unimplemented marker occurs in the changed WP-1 product files. No empty implementation or network client was found.

The adversarial checks specifically challenged three plausible false positives:

1. A passing parser suite could have hidden a broken real runner; this was the original HIGH-01. Final code passes the authoritative inventory into the real subprocess parser, and Green plus Red are asserted on nonzero exits.
2. Passing ordinary Git fixtures could have inherited a hostile object database; this was HIGH-02. Final probes start from `env_clear`, and the foreign-object regression passes.
3. `store_replace_overwrites_existing_atomically` alone does not prove the OS primitive by assertion wording; direct implementation inspection confirms the exact IFACE §9 Windows/Unix primitives and no remove-before-rename fallback.

One test name is broader than its assertion: `mint_outside_repo_never_claims_verified` proves out-of-repo preflight is `Unverifiable`; the stronger structural claim is separately proved by `mint_receipt` returning only `Receipt` and `ReceiptPreflight` having no `Valid` arm. This is not a coverage gap.

## Human Verification Required

None. Every behavior-dependent Phase-4 truth has a passing executable test or deterministic implementation/receipt oracle; no UI, external service, or subjective behavior is part of WP-1.

## Gaps Summary

No Phase-4 goal gap remains. WP-1 is locally verified and ready for the separate strength-gated detached integration/push/CI promotion sequence. This report does not claim that promotion has occurred.

---

_Verified: 2026-08-20T05:45:00Z_
_Verifier: the agent (ferrox-verifier)_
