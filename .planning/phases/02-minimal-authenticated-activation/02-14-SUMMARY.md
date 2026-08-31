---
phase: 02-minimal-authenticated-activation
plan: 14
subsystem: delegated-activation-effects
tags: [mcp, tasks, durable-effects, authority-narrowing]
requires: [02-13]
status: complete
---

# Phase 2 Plan 14 Summary

Actual MCP remote calls and child creation now consume admitted, currently enabled authority before external work and cannot redispatch an ambiguous effect after rebuild.

## Outcome

- MCP tool/resource calls use stable activation/call/server/tool identities, journal-first intent, result digest and `unknown_outcome` recovery.
- Task/review spawn paths check exact-artifact enablement, deadline, issuer/key/project/principal epochs and durable controls before workspace copy/thread creation.
- Child capabilities remove `task.spawn`; turn/tool/wall budgets are strictly smaller and both advertised/runtime tools are narrowed.
- Real stdio MCP and task-directory oracles prove default-off zero effect, signed-control zero effect, one dispatch before ambiguity and zero redispatch after reconstruction.

## Commit

- `4dd42c8 feat(auth): mediate delegated effects`

## Verification

- delegated MCP/task tests: 5/5 passed
- nano-agent library: 308/308 passed
- full affected nano-agent suite passed before final capability refinement; final refinement reran 308 library + 5 delegated tests
- clippy `-D warnings`: passed
- fmt and diff checks: passed

## Deviations

- Enforced the delegated capability/budget subset inside child runtime rather than hashing proposal only; otherwise children could retain wider tools.
- Preserved a deterministic test-only receipt key clone for signed-control evidence.
- Generated crate-local `target/` was left untracked because environment deletion policy rejected recursive cleanup; it is not in Git.

## Self-Check: PASSED

- Four plan-owned source/test files committed.
- No ACP reader, quarantine, Desktop or Phase 3 source changed.
