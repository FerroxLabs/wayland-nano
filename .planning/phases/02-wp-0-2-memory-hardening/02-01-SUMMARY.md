---
phase: 02-wp-0-2-memory-hardening
plan: 01
subsystem: memory-instrumentation
tags: [mem-stats, mcp-accounting, canary, authorization]
requires: [DEV-WP-0.2A, DEV-WP-0.2B]
provides: [feature-gated-memory-reporter, recursive-mcp-accounting, exact-list-canary]
affects: [02-02, 02-03, 02-04, 02-05]
tech-stack:
  added: []
  patterns: [closed-numeric-schema, append-only-ndjson, governed-exact-list-scan]
key-files:
  created: []
  modified:
    - docs/FOLLOWUPS.md
    - crates/nano-agent/src/mcp.rs
    - crates/nano-agent/src/mcp_tests.rs
    - crates/nano-cli/Cargo.toml
    - crates/nano-cli/src/acp_mode.rs
    - scripts/canary/scan.mjs
decisions:
  - "Enable the reporter only through the off-by-default mem-stats feature and NANO_MEM_STATS path."
  - "Account recursively retained MCP registry state and preserve sessions_map as the host's 0-or-1 Option<Session> cardinality."
metrics:
  completed: 2026-08-16
status: complete
---

# Phase 2 Plan 01: Memory measurement surface summary

Owner-authorized, feature-gated retained-memory instrumentation emits the locked numeric NDJSON schema while exact-list canary scanning and recursive MCP accounting make later profile evidence independently checkable.

## Results

- `a2c8e6d` recorded the signed DEV-WP-0.2A measurement authorization and DEV-WP-0.2B scanner-slice authorization before implementation.
- `18e88e7` added recursive retained-byte accounting for the MCP registry and focused `nano-agent` coverage.
- `1de46df` added the governed exact-list scanner and receipt surface with synthetic self-tests.
- `4a53c86` added the off-by-default `mem-stats` feature and the `NANO_MEM_STATS` reporter, including the closed numeric schema, 25-turn cadence, startup/write policy, protocol-channel isolation, Windows private-working-set sampling, and 0-or-1 `sessions_map` accounting.
- `beeb5e2` corrected governed key resolution across linked worktrees before profiling began. It is pre-profile implementation work, not part of the later Critical/High fix round.

## Verification Evidence

Retained Phase 02 summaries, the builder handoff, and the final Plan 06 reruns support these gates:

- `cargo test -p nano-cli acp_mode::tests::mem_stats --features mem-stats`: five mem-stats tests pass.
- `node --check scripts/canary/scan.mjs`: scanner syntax passes.
- `node scripts/canary/scan.mjs --self-test-include-list`: governed exact-list synthetic self-test passes.
- `cargo check -p nano-cli --all-targets`: the default all-target compilation passes with reporter behavior compiled out.
- `cargo build --release -p nano-cli -F nano-agent/soak-fake-model -F nano-cli/mem-stats`: the combined-feature release build passes.
- Relevant `nano-agent` coverage for recursive MCP accounting passes, and the later full workspace gate exercises the complete workspace suite.

No RED-phase result, duration, or unsupported count is reconstructed here; this summary records only inspected commits and retained/rerun evidence.

## Deviations from Plan

None in the reconstructed implementation record. The required summary was missing from the original closeout and is restored by Plan 06 from repository evidence only.

## Decisions Made

The reporter remains opt-in through both the compile-time feature and `NANO_MEM_STATS`; records contain numeric retained-state measurements only. Recursive MCP accounting is measurement instrumentation. The `beeb5e2` worktree-resolution correction predates profiling and therefore predates the bounded final audit/fix chronology.

## Self-Check: PASSED

All five named commits exist, their changed-file sets and subjects were inspected, and every test claim above is supported by retained summaries/handoff evidence and the Plan 06 reruns.
