# Constraints

## Memory content schema and identity
- source: .planning/sources/MEMORY-CONTRACT.md
- type: schema
- content: Episodes, facts, decisions, procedures, and populated KG rows carry immutable `project` and `agent_id`; content rows use `agent_id TEXT NOT NULL DEFAULT 'main'`; identity remains separate from write provenance; facts are bi-temporal and preserve supersession history.

## T2 storage
- source: .planning/sources/MEMORY-CONTRACT.md
- type: schema
- content: The store is one local `<nano_home>/memory/memory.db` using bundled SQLite, FTS5, and sqlite-vec; SQLite is a rebuildable index, not authority; services, daemons, graph databases, and network filesystems are excluded.

## Retrieval pipeline
- source: .planning/sources/MEMORY-CONTRACT.md
- type: protocol
- content: Retrieval executes separate project- and agent-filtered FTS5 and KNN passes, RRF k=60, tier down-weighting, diversity, pre-output privacy/retention filtering, token trimming, and provenance. KG-BFS is a separately built, fixture-gated third leg with depth <= 2 and <=7k retrieved tokens.

## Agent scope
- source: .planning/sources/MEMORY-CONTRACT.md
- type: api-contract
- content: Every query requires project and agent scope. Agent scope is Own, OwnAndProject, or an explicit agent-id list; predicates apply inside retrieval passes and are asserted again on assembled output. No cross-agent read ships before its later package.

## Core embedder
- source: .planning/sources/MEMORY-CONTRACT.md
- type: api-contract
- content: Hashed-local 384-dimensional embedding is the only core backend and must remain behind the Embedder trait; hosted embedding is a separate committed package.

## Journal-first durability
- source: .planning/sources/MEMORY-CONTRACT.md
- type: protocol
- content: Every memory write journals first with trust, project, and agent identity preserved bit-for-bit; DB recovery is drop-and-rebuild from journals; one shared database uses session locking and sequential per-agent activation; migrations are additive within a major version.

## Memory policy
- source: .planning/sources/MEMORY-CONTRACT.md
- type: api-contract
- content: MemoryPolicy contains enabled, write scope, Session-or-SessionAndProject read scope, retention, embedder choice, deletion rule, and minimum trust tier. Profiles may only tighten it; unknown enum values fail with typed errors; Global is absent in v1.

## Trust assignment and contradiction resolution
- source: .planning/sources/MEMORY-CONTRACT.md
- type: protocol
- content: Source trust is User, ToolOutput, or ModelInference; ambiguous and model-mediated content resolves to ModelInference. Conflict domain is `(project, agent_id, subject, predicate)`; higher trust always wins, lower trust never supersedes, and equal-tier resolution uses the pinned confidence-based 1.2x donor rule with deterministic exact-tie behavior.

## Retrieval trust weighting
- source: .planning/sources/MEMORY-CONTRACT.md
- type: protocol
- content: After fusion, User scores multiply by 1.0, ToolOutput by 0.8, and ModelInference by 0.5 before diversity and trimming; MemoryPolicy minimum tier excludes lower tiers.

## Write authority
- source: .planning/sources/MEMORY-CONTRACT.md
- type: protocol
- content: Models propose memory writes and the host validates and commits them; model-initiated writes are capped at ModelInference, committed writes are journaled before indexing, and visible receipts bind proposal, outcome, scope, identity, and resolver result.

## Agent identity grammar and registry
- source: .planning/sources/MEMORY-CONTRACT.md
- type: api-contract
- content: One canonical agent-id grammar applies across config, journal, DB, CLI, task, routine, and execution surfaces. IDs validate against `$NANO_HOME/agents/*.agent.toml`; `main` is implicit; unknown IDs fail closed; retired IDs are never recycled.

## Persona core boundary
- source: .planning/sources/MEMORY-CONTRACT.md
- type: protocol
- content: The kernel-owned prompt core is immutable and always first; persona content is an overlay that cannot replace it or widen capabilities, memory policy, egress, approvals, or tool grants; loading and hashing fail closed.

## Memory acceptance bars
- source: .planning/sources/MEMORY-CONTRACT.md
- type: nfr
- content: Acceptance requires recall@10 >= 0.90 with zero project/agent leakage, journal-authoritative kill recovery, the adversarial mem-sec pack, mediated write proof, retention and partition enforcement, and seven-leg CI including Windows ARM64.

## Profile closed schema
- source: .planning/sources/PROFILES-CONTRACT.md
- type: schema
- content: Profile v1 is a closed TOML structure; unknown fields and unknown profile names fail closed. It includes version, name, extends, mode default/ceiling, tool allow/deny, approval overrides, task budget, and system prompt overlay reference.

## Profile merge and selection
- source: .planning/sources/PROFILES-CONTRACT.md
- type: protocol
- content: Mode and budget use narrowing lattice operations, allowlists intersect, denylists union, extends is narrow-only and acyclic, selection precedence is CLI then trusted project then user config then builtin balanced, and launch posture remains an upper bound.

## Profile journaling and resume
- source: .planning/sources/PROFILES-CONTRACT.md
- type: protocol
- content: Resolved profile identity, canonical fields, source, and composition hash are journaled at session start; resume recomputes current posture and narrows against the journaled posture so revoked authority cannot return.

## Module manifest
- source: .planning/sources/NANO-MODULE-CONTRACT.md
- type: schema
- content: A v1 module is a directory containing `module.toml` and a payload; manifest identity, source pin, server/skill lists, requested capabilities, platform constraints, and versioned contract are declarative and closed.

## Module lifecycle and trust
- source: .planning/sources/NANO-MODULE-CONTRACT.md
- type: protocol
- content: Registration is explicit, validated, receipt-bearing, and fail-closed; discovery does not execute payloads; module registration cannot grant capability beyond the active policy lattice; unregistration is auditable and preserves historical receipts.

## Module v1 exclusions
- source: .planning/sources/NANO-MODULE-CONTRACT.md
- type: nfr
- content: No runtime plugin ABI, arbitrary lifecycle hooks, hidden installers, automatic network fetch, registry governance design, or capability widening belongs in module v1.
