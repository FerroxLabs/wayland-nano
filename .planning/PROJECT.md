# Wayland Nano Verified Change

## Authority

This file is a Ferrox navigation projection. It does not replace the program
documents. The single entry point is
`../shared/reviews/research-0.2/NANO-BUILD-PLAN-V3.md`. Its standing execution
rules are binding. For WP-1 through WP-4, the authority order is:

1. `../shared/reviews/research-0.2/specs/SPEC-WP-INTERFACES.md`
2. The work-package spec referenced by the master document
3. `../shared/reviews/research-0.2/NANO-BUILD-PLAN-V3.md`
4. `../shared/reviews/research-0.2/GOALS.md`

Any contradiction is resolved by the higher authority. A boundary conflict
stops that sub-task and produces the required deviation note; it never licenses
an improvised workaround.

## What This Is

Wayland Nano is a small native Rust execution runtime whose current product
program turns its proof discipline into `wayland-nano verify`: executable gates
that reject fluent-but-wrong agent changes plus independently rerunnable
red-green receipts. The customer is an engineering organization allowing agents
to modify real repositories and needing an enforceable merge condition.

## Core Value

A change earns trust only through independently rerunnable machine evidence;
model confidence and self-report never substitute for a green gate.

## Business Context

- **Customer**: Engineering teams allowing coding agents to change real repositories.
- **Success metric**: At least two external teams voluntarily require the Wayland receipt as a merge condition.
- **Strategy notes**: `../shared/reviews/research-0.2/NANO-FINAL-PLAN-V2.md`

## Requirements

### Validated

- ✓ Native Rust runtime, fail-closed containment, typed policy seams, append-only session journal, and provider-neutral execution loop — existing v0.1.1 product.
- ✓ Attested multi-target release pipeline consumed by Wayland Desktop — existing v0.1.1 release.
- ✓ Full workspace quality gate: formatting, clippy with warnings denied, and workspace tests — existing repository contract.

### Active

- [ ] Complete Phase 0 hardening: WP-0.2, WP-0.3, and WP-0.4.
- [ ] Build the gate runner and red-green receipt foundation in WP-1.
- [ ] Build the budgeted gated-climb engine in WP-2.
- [ ] Build the `wayland-nano verify` CLI, CI consumer, and offline verifier in WP-3.
- [ ] Build and dogfood the three sealed Gate Card packs in WP-4.
- [ ] Produce canary-clean, reproducible evidence for every promoted claim.

### Owner-Led / External Gates

- WP-0.1 is an interactive host-run proof requiring the owner's Windows desktop and manual 100%/150% display scaling.
- WP-5 partner demo/onboarding and WP-6 adoption decision are owner-led and are not autonomous build phases.

### Out of Scope

- WP-5 and WP-6 execution — stop after WP-4 and hand control to the owner.
- P-MEM, P-PROF, P-MOD, MCP server mode, self-evolution, and subscription bridging — frozen until WP-6 succeeds.
- Writes to `../nano/` or `../resources/upstreams/` — immutable/read-only boundaries.
- Speculative features, drive-by refactors, dependency upgrades, unrelated cleanup, and sidequests — every changed line must trace to the active WP.

## Execution Model

- Execute one WP promotion at a time in the master document's dependency order.
- Use a dedicated worktree and branch from current `origin/master` for every WP.
- Use parallel subagent swarms only for independent research, planning, audits,
  verification, or explicitly disjoint owned files.
- Serialize edits to hot seams including `acp_mode.rs`, `crates/nano-verify/**`,
  generated artifacts, and integration state.
- Give every builder explicit file ownership and an isolated worktree. Builders
  never merge or push.
- Per WP: implement within OWNS/NEVER-TOUCH, run one Critical/High audit, one fix
  round, fix verification, and the complete local gate.
- Integrate one branch at a time through detached `.tmp-wt-integ` using
  `--no-ff`; re-run the full gate, push `HEAD:master`, and require CI green before
  promoting the next dependency.
- Report one line per WP: WP, commits, local/integration gate, and CI result.

## Constraints

- **Security**: Fail closed; never weaken a security invariant or test to pass.
- **Secrets**: The Flux key is path-only at `../.secrets/flux-test-key`; never read, echo, copy, or embed it. Canary-scan captured evidence.
- **Generated artifacts**: Regenerate error tables with `cargo run -p nano-cli --bin gen_error_table`; never hand-edit them or change their mirror without regeneration.
- **Dependencies**: No new dependency without a cargo-deny-clean justification.
- **Evidence**: Keep Implemented, Reachable, and Live-proven claims separate.
- **Platform gate**: Three Windows `SetNamedSecurityInfoW` ACL failures may be environmental; report them and never weaken or chase the tests.
- **Baseline truth**: Build from current `origin/master`; stale `.tmp-wt-*` worktrees are never source truth.

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Lead with verified change rather than memory | Trust and production accountability are the wedge; adoption must earn the broader roadmap | — Pending WP-6 |
| Canonical interface contract wins | Prevents independently authored WP surfaces from drifting | — Binding |
| Parallelize analysis, serialize hot seams and promotion | Gains swarm speed without semantic merge guesses or branch chaos | — Binding |
| Stop autonomous execution after WP-4 | Demo partnerships and the adoption decision require owner leadership | — Binding |

## Evolution

This projection changes only when the authoritative program changes. Phase
completion may update requirement status and evidence pointers but may not add
scope. New capabilities require an explicit master-plan amendment before they
enter the roadmap.

---
*Last updated: 2026-08-16 after Ferrox brownfield onboarding*
