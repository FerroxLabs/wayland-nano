---
phase: 06-wp-3-verify-cli-and-ci-surface
plan: 04
subsystem: cli
tags: [rust, git-worktree, provider-routing, nano-verify, fail-closed]
requires:
  - phase: 06-wp-3-verify-cli-and-ci-surface
    provides: closed verify modes, registry loading, bounded Git probes, and receipt checking
provides:
  - Exact-start detached Red baseline orchestration with identity and cleanup proof
  - Public provider-router production Effects adapter for both catalog wire kinds
  - Caller-ordered climb configuration under one absolute deadline
affects: [06-05, 06-06, CLI-02, CLI-03, CLI-05]
tech-stack:
  added: []
  patterns: [detached exact-tree evidence, catalog-derived egress, TextDelta-only generation]
key-files:
  created: [.planning/phases/06-wp-3-verify-cli-and-ci-surface/06-04-SUMMARY.md]
  modified: [crates/nano-cli/src/verify_cmd.rs]
key-decisions:
  - "Treat only full-inventory Red with a nonzero exit and lowercase SHA-256 log digest as receipt-eligible baseline evidence."
  - "Resolve every CLI-supplied model independently through ProviderRouter and construct egress solely from the returned catalog binding."
patterns-established:
  - "Baseline evidence is captured only in a detached exact-start worktree and survives source/detached identity checks plus cleanup proof."
  - "Production Effects owns generation, typed engine events, monotonic time, and cancellation state only."
requirements-completed: [CLI-02, CLI-03, CLI-05]
coverage:
  - id: D1
    description: Exact-start detached Red baseline eligibility and cleanup orchestration
    requirement: CLI-02
    verification:
      - kind: unit
        ref: "verify_cmd::tests::baseline"
        status: pass
      - kind: other
        ref: "just gate-all"
        status: pass
    human_judgment: false
  - id: D2
    description: Public provider adapter and caller-ordered climb configuration
    requirement: CLI-02
    verification:
      - kind: unit
        ref: "verify_cmd::tests::mint_flow"
        status: pass
      - kind: other
        ref: "cargo clippy -p nano-cli --all-targets -- -D warnings"
        status: pass
    human_judgment: false
duration: 31min
completed: 2026-08-21
status: complete
---

# Phase 6 Plan 4: Production Effects and Detached Baseline Summary

**Mint orchestration now proves Red on an exact-start detached tree, then drives the climb through public catalog-bound provider adapters with caller-controlled model order and one absolute deadline.**

## Performance

- **Duration:** 31 min
- **Completed:** 2026-08-21
- **Tasks:** 2 TDD tasks
- **Files modified:** 2

## Accomplishments

- Added strict source and detached HEAD/tree/status guards around baseline execution, full-inventory Red eligibility checks, nonzero exit and lowercase digest requirements, and mandatory worktree removal/prune/absence proof.
- Added production generation through `ProviderRouter::from_env` and `resolve_binding`, catalog-derived egress, both public provider wires, one-user/no-tools/non-stream requests, and TextDelta-only candidate collection.
- Wired zero-argument sealed artifact-workspace creation and `run_climb` with the exact cheap model, ordered escalation models, caller budget, and command-entry absolute deadline.

## Task Commits

1. **RED: baseline and mint adapter contracts** - `811ff66`
2. **GREEN: detached baseline and model Effects** - `ddaee58`

## Files Created/Modified

- `crates/nano-cli/src/verify_cmd.rs` - detached baseline transaction, production Effects adapter, climb configuration, and focused tests.
- `.planning/phases/06-wp-3-verify-cli-and-ci-surface/06-04-SUMMARY.md` - execution evidence and requirement traceability.

## Decisions Made

- Provider diagnostics and all router, credential, egress, client, driver, and stream failures collapse to the fixed internal `generation_failed` code; provider text and identity never enter engine errors or CLI events.
- The baseline invocation is discarded with its detached tree; climb receives a freshly reconstructed registry invocation rooted at the clean source checkout.
- Verified accepted artifacts stop at the explicit Plan 06-05 materializer boundary; Plan 04 does not inspect or install provider bytes.

## Verification

- `cargo test -p nano-cli verify_cmd::tests::baseline --lib -- --nocapture` - 1 passed.
- `cargo test -p nano-cli verify_cmd::tests::mint_flow --lib -- --nocapture` - 4 passed.
- `cargo test -p nano-cli verify_cmd::tests --lib -- --nocapture` - 24 passed.
- `cargo clippy -p nano-cli --all-targets -- -D warnings` - passed.
- `just gate-all` under F:-only TEMP/TMP/CARGO_TARGET_DIR - passed.

## Deviations from Plan

None - executed the Plan 04-owned detached-baseline, production-Effects, and climb boundary exactly; coherent materialization/commit/rerun/store remains Plan 06-05's owned downstream seam.

## Known Stubs

None in the Plan 04-owned provider or detached-baseline surface. The deliberate verified-artifact handoff to Plan 06-05 is fail closed (exit 3) until that dependent plan connects the materializer transaction.

## Threat Flags

| Flag | File | Description |
|---|---|---|
| threat_flag: local-git-worktree | `crates/nano-cli/src/verify_cmd.rs` | Exact-start detached baseline is bounded, identity checked, and unconditionally cleaned. |
| threat_flag: provider-egress | `crates/nano-cli/src/verify_cmd.rs` | Provider dispatch uses only public catalog bindings and deny-by-default catalog-derived egress. |

## Self-Check: PASSED

- Product file and summary exist.
- RED and GREEN commits exist on `worktree-agent-wp3-04`.
- Focused tests, strict Clippy, and the full workspace gate passed on final product bytes.
- No file outside the assigned Plan 04 product and summary paths changed.
