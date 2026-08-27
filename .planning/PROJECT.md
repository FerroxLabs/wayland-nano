# Wayland Nano

## Core Value

A change earns trust only through independently rerunnable machine evidence.

## Current Milestone: v1.1 Workable Persistent Agent

**Goal:** A Desktop-defined bot can repeatedly invoke Nano with authenticated, project/principal-scoped continuity and bounded host-triggered work. Desktop remains the sole product control plane; Nano remains the security/continuity/execution kernel.

This is the smallest useful persistent-agent slice. Hardened browser and desktop providers are separate later milestones and do not block it.

## Boundary

- Desktop owns bot→principal binding, bot CRUD/registry, persona/team/model/backend choices, schedules/timers, approvals, history/attention UI, and later provider selection.
- Nano trusts only an explicitly configured issuer assertion. It validates issuer, freshness, replay protection, local policy intersection, memory scope, bounds, journals, and receipts. It cannot prove the real-world truth of Desktop's mapping. Minimal Nano security enrollment/binding state may enforce trust roots and anti-remap; it is not a product registry.
- ACP is transport, not semantic authorization or orchestration.
- For v1.1 compatibility, wire `principal_id` is the semantic name for the immutable authority partition and maps 1:1 to the existing physical/schema/journal `agent_id`. No schema rename occurs in this milestone.

## Authority and Source Status

`.planning/sources/` contains hash-verified immutable snapshots. MEMORY-CONTRACT v1.2 is owner-signed. NANO-PROGRAM-PLAN, PROFILES-CONTRACT, and NANO-MODULE-CONTRACT are authoritative snapshots but are not represented here as owner-signed. ADR-001 is only a proposed amendment.

Only PR #8 acceptance is eligible until the owner signs/version-stamps a ratification manifest listing every source artifact/version/signature/disposition/precedence and pinning ownership; trust roots/key lifecycle; immutable bot→principal binding; descriptor/replay; `principal_id`↔`agent_id` compatibility; protected MEM-SEC fixture ownership; merge order and compatibility window. Rejection or ambiguity stops and re-roadmaps.

## Four Focused Phases

1. Ratify the boundary and accept P-MEM-1.
2. Add the minimal authenticated activation/policy contract.
3. Wire secure scoped memory into the real runtime and measure continuity.
4. Run bounded Desktop-triggered work and immediately dogfood/decide.

## Cross-Repo Authority

This Nano worktree may define the protocol and Nano implementation only. Its `AGENTS.md` excludes Desktop writes. Any Desktop implementation requires a separately authorized Desktop worktree/branch/PR or an exact owner handoff. Cross-repo acceptance uses pinned fixtures, declared merge order, and a bounded compatibility window; it never edits Desktop from this worktree.

## Later Focused Milestones

- v1.2 Hardened Browser Provider
- v1.3 Hardened Desktop Provider
- Evidence-gated backlog: compaction/procedure extraction, code graph/blast/KG, and cross-project reads

## Discipline and Out of Scope

One phase/goal at a time; isolated worktree; no side quests; three strikes; exact handoff on stop; human review; governing local/CI evidence. No Nano bot registry, persona/team system, scheduler, UI, provider work in v1.1, hosted memory, schema rename, composition-digest framework without an enforcement consumer, extraction, graph, KG, or cross-project reads.
