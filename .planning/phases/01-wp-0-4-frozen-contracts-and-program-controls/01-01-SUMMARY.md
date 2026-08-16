---
phase: 01-wp-0-4-frozen-contracts-and-program-controls
plan: 01
subsystem: contracts
tags: [rust, canonical-json, frozen-contracts]
requires: []
provides:
  - Four repository-root frozen contract artifacts and prose siblings
  - Deterministic source-derived contract generator
  - Exhaustive journal Op vocabulary guard
affects: [wp-0-4, contract-validation, promotion]
tech-stack:
  added: []
  patterns: [mandatory root authority, byte-canonical JSON, generator-only outputs]
key-files:
  created: [crates/nano-cli/src/bin/gen_contracts.rs, contracts/capability-profile.json, contracts/journal-semantics.json, contracts/flux-endpoint-contract.json, contracts/event-types.json]
  modified: [crates/nano-session/src/op.rs, README.md, docs/STATUS.md]
key-decisions:
  - "Repository-root contracts are mandatory; external shared mirrors are non-authoritative."
requirements-completed: [CTRL-01, CTRL-02, CTRL-03, CTRL-06, CTRL-07, CTRL-08, HOST-01, CTR-01, CTR-02, CTR-05, CTR-06]
duration: 1h
completed: 2026-08-16
status: complete
---

# Phase 1 Plan 01: Frozen Contracts Summary

**Four canonical root contracts with deterministic Rust/corpus derivation and exhaustive journal vocabulary coverage**

## Accomplishments

- `28353cd` added the 37-tag sorted `OP_VOCABULARY` and exhaustive serde representative test.
- `2437f16` added `gen_contracts`; consecutive generation hashes were stable, clean `--check` passed, and a one-byte mutation produced `STALE`/exit 1 before regeneration restored the artifact.
- `5c1165e` added the six-endpoint evidence contract, authoritative prose siblings, and exact root-location documentation.
- Planning corrections `baabef6` and `0143f89` established tracked root authority and corrected the historical documentation baseline used by verification.

## Verification

- `cargo test -p nano-session op_vocabulary`: 1 passed, 0 failed.
- `cargo run -p nano-cli --bin gen_contracts -- --check`: all three generated root artifacts current.
- Endpoint JSON: six complete unique method/path records, canonical bytes, no trailing newline.

## Deviations from Plan

- Initial external `shared/contracts` ownership was uncommittable; blocker `f184c4a` and correction `baabef6` moved authority to tracked root `contracts/`.
- No product behavior was weakened.

## Owner Actions

- HOST-01: WP-0.1 was not executed; interactive Windows CUA proof remains owner/host-run.
- CTR-06: G-CTR-1 is ready for narrow owner/integrator review but remains unapproved; the executor did not edit the catalog.

## Self-Check: PASSED

All declared artifacts and commits (`28353cd`, `2437f16`, `5c1165e`) exist.
