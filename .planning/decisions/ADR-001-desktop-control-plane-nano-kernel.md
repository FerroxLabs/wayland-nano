# ADR-001: Desktop Control Plane, Nano Kernel

**Status:** Proposed owner amendment; not governing
**Date:** 2026-08-27

## Source Status

All `.planning/sources/` files are hash-verified immutable snapshots. MEMORY-CONTRACT v1.2 is owner-signed. NANO-PROGRAM-PLAN, PROFILES-CONTRACT, and NANO-MODULE-CONTRACT are authoritative snapshots but not represented as signed. Ratification must inventory every artifact/version/signature/disposition/precedence and protect the MEM-SEC fixture amendment; this ADR alone cannot supersede them.

## Proposed Decision

Desktop owns product bot identity/CRUD and mapping. Nano verifies issuer assertions, not their real-world truth, and may keep minimal trust-root/anti-remap security enrollment state; that state is not a product registry.

For v1.1, wire `principal_id` is a semantic alias mapped 1:1 to immutable physical/schema/journal `agent_id`; no rename. Bot reassignment/remap is forbidden unless a future signed migration policy defines a new principal and auditable transfer.

The minimal descriptor carries principal/project, optional audit-only product reference, activation/session/idempotency IDs, continuity, requested capabilities, budgets/deadline, replay protection, and a resume-context fingerprint. That fingerprint covers only policy, tool, persona, and module references needed to detect resume authority/context drift; drift fails closed or explicitly falls back to fresh. Broader composition digests remain additive only with an enforcement consumer.

Direct CLI admission uses a locally enrolled issuer and an explicit `main` compatibility principal under the same trust, replay, and identity rules. CLI invocation is never an issuer or principal bypass.

Nano stores no schedules or timers. Desktop alone fires activations. Idempotency covers admission, journal/memory/receipt commit, and effect-dispatch identity—not inherently external effects. Ambiguous effects use an intent ledger and typed `unknown_outcome`/manual reconciliation unless a provider proves idempotent commit.

## Ratification Must Pin

- Trusted issuer/trust root and provisioning, rotation, overlap, revocation, recovery
- Desktop bot→principal authority, immutable mapping, never-remap/reassignment rule, negative tests
- Carrier/version/downgrade/replay rules through authoritative Desktop `AcpConnection`
- Existing `agent_id` storage/journal compatibility, rebuild/query equivalence, receipt fields
- Fixture owner, exact Nano artifact SHA, merge order, compatibility window and exit criteria
- Inventory/migration/disablement of existing Nano cron and timer/tool advertisement

## Repository Authority

This Nano repository may define the protocol and Nano side. Its `AGENTS.md` excludes Desktop writes. Desktop implementation requires a separately authorized Desktop worktree/branch/PR or exact owner handoff.

## Deferred

Browser v1.2 and desktop v1.3 are independently promoted. Each must prove ordinary Playwright MCP or host CUA cannot bypass Nano and must carry a dedicated Docker/Podman evidence matrix. Compaction/procedures, graph/blast/KG, and cross-project reads remain evidence-gated.
