# Onboarding Summary

## Current State

- Active milestone: v1.1 Workable Persistent Agent
- Four focused phases, nine active requirements mapped exactly once, zero plans and 0% progress
- Phase 1 waits for the owner ratification manifest and PR #8 human merge/fresh verification
- Signed/hash-verified source status is explicit; snapshots remain untouched

## Architecture

Desktop is the product control plane. Nano verifies a trusted issuer assertion, narrows authority, provides project/principal-scoped continuity, enforces bounds, and emits evidence. Wire `principal_id` maps 1:1 to existing immutable physical `agent_id` for v1.1. Nano security enrollment is allowed only for trust-root and anti-remap enforcement; it is not a bot registry.

## Cross-Repo Boundary

The Nano worktree cannot edit Desktop. Desktop integration through authoritative `AcpConnection` requires separate owner authorization/worktree/PR or exact handoff. Contract tests pin fixture owner, merge order, compatibility window, carrier/version/downgrade, job occurrence derivation, and exact Nano artifact SHA.

## Active Sequence

1. Ownership contract and foundation
2. Minimal authenticated activation
3. Runtime-integrated scoped continuity
4. Bounded Desktop trigger and immediate dogfood decision

Browser and desktop providers are subsequent v1.2/v1.3 milestones with bypass-negative and Docker/Podman gates. Extraction, graph/KG, and cross-project reads remain evidence-gated.

## Next Step

Complete Phase 1 only: ratify the source/authority/identity/fixture/compatibility manifest, merge PR #8, and freshly verify its evidence.
