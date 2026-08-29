# Requirements

## REQ-wp-0-contract-freeze
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Owner review and signature of MEMORY-CONTRACT v1.2, plus remaining WP-6 Phase-0 housekeeping.
- acceptance: Signed contract; CODEOWNERS protection verified for `gates/**` and `agents/**`.
- scope: WP-0 contract freeze

## REQ-p-mem-1-core-memory-store
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Build the `nano-memory` SQLite/FTS5/sqlite-vec core with bi-temporal, project- and agent-scoped content, deterministic tier-aware resolution, journal-first writes, retrieval passes 1–2 with RRF, mediation, keyed retention, and unpopulated KG tables.
- acceptance: Recall@10 >= 0.90 on `memory-retrieval-recall-v1` with zero cross-project and cross-agent rows; kill-mid-write rebuild is query-equivalent including `agent_id`; write mediation passes; all seven CI legs pass.
- scope: P-MEM-1 core memory store

## REQ-p-mem-sec-gate-pack
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Implement the six MEMORY-CONTRACT §6.5 adversarial memory cards on the existing verification and gate-card machinery.
- acceptance: The pack passes on all seven CI legs and the `gates/**` change receives required owner review; isolation is asserted at pass 1, pass 2, and assembled output.
- scope: P-MEM-SEC memory security

## REQ-p-prof-profiles
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Implement the closed profile schema, deterministic merge math, selection, `Op::ProfileSet`, resume-narrows behavior, three shipped profiles, and overlay-only system prompt semantics.
- acceptance: Merge-math, fail-closed resolution, journal roundtrip, resume-narrows, shipped-profile behavior, and `just gate-all` tests pass.
- scope: P-PROF profiles

## REQ-p-mod-gap-manifest-integrity
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Add module `contract_version`, digest-pinned source variants with install-time verification, install receipt journaling, provenance wiring, and typed refusal for ungoverned registry sources.
- acceptance: Tampered digests fail closed; install receipts contain the digest; registry-kind manifests return the typed refusal; `just gate-all` passes.
- scope: P-MOD-GAP module manifest gap closure

## REQ-p-bot-5a-agent-composition
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Add named agent registry/configuration, spawn-by-agent identity, journaled resolved composition, ceiling intersection, persona overlay, identity-bearing usage rollups, host-side roster primitives, scaffold command, and the assistant/reviewer/researcher dogfood trio.
- acceptance: Spawn-by-name journals the composition hash; untrusted project agents refuse; ceiling narrowing and core-preserving persona overlay are proven; rollups carry `agent_id`; `agents/**` is review-protected.
- scope: P-BOT-5a named agent composition

## REQ-p-bot-5b-persistence
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Add memory-primary agent continuity with journal-chain audit fallback, identity-checked activation, re-derived posture, single live activation, scoped recall, mediated `memory_propose`, per-agent ledger folding, visible memory receipts, and export-before-prune GC.
- acceptance: Continuity is reconstructible with digest checks; revoked capabilities stay revoked; MEM-SEC-6 passes; per-agent rollback works; parent journal alone reconstructs ledger output.
- scope: P-BOT-5b persistent accumulation

## REQ-p-bot-5c-proactivity
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Add agent-addressed routines, memory-primary routine activation, bounded routine/run retention, immutable prompt-snapshot receipts, metering and denials, rate-limited attention requests, typed activation failures, bounded retry, and an agent pause switch.
- acceptance: A routine activates an agent and records required receipts; escalation is journaled and rate-limited under loop-spin testing; failure reasons are typed; spin caps are enforced in code.
- scope: P-BOT-5c routines and escalation

## REQ-p-exe-1-browser-backend
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Add the execution-backend composition seam and hardened browser-container flavor with constrained supervisor vocabulary, digest-pinned images, inspect-time posture verification, CDP observation, frame receipts, leases, capability intersection, and human-control refusal.
- acceptance: Supervisor vocabulary, tamper refusal, image provenance, historical frame receipts, and vision-token metering are proven.
- scope: P-EXE-1 browser execution backend

## REQ-p-exe-2-desktop-backend
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Add the hardened XFCE/VNC desktop-container flavor on the P-EXE-1 seam with recreate-not-resume, AX-first actions, and who-is-driving control.
- acceptance: Per-agent backend selection is journaled; recreate-not-resume is typed; the shared hardening inspection passes on Docker and Podman.
- scope: P-EXE-2 desktop execution backend

## REQ-p-cons-memory-compaction
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Make compaction extraction functional, extracting facts and decisions through host mediation with ModelInference cap and visible receipts, while deciding retention and extraction-model cost in the package design phase.
- acceptance: Mediated extraction is journaled and receipted; compacted facts remain retrievable; retention caps are enforced; the extraction-cost comparison is reported.
- scope: P-CONS memory-aware compaction

## REQ-p-proc-procedure-extraction
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Mine repeated successful journal shapes into proposed, mediated, ModelInference-tier procedures scoped by project and agent, then retrieve them through existing passes.
- acceptance: The fixture yields a proposal; host commit produces a receipt; later same-scope retrieval succeeds; other scopes cannot see it; no receiptless procedure lands.
- scope: P-PROC learn-from-doing

## REQ-p-graph-1-code-index
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Replace regex repomap internals with a tree-sitter def/ref index and budget-ranked serialization, including incremental indexing, cache invalidation, tracked-file scope, and RSS budgets.
- acceptance: Localization benchmark is committed; serialization respects token budget; edit-churn reindex is correct; CI RSS budgets are enforced.
- scope: P-GRAPH-1 code-structure index

## REQ-p-graph-2-blast-radius
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Add Rust-only call-edge extraction as a suggested blast radius whose displayed confidence is derived from the precision evaluation; never present it as asserted fact.
- acceptance: Precision evaluation and fixture are committed; every result carries measured confidence; a losing evaluation ships the capability dark with the negative result documented.
- scope: P-GRAPH-2 suggested blast radius

## REQ-p-mem-kg-retrieval
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Build the thin KG-BFS third retrieval leg with deterministic nodes/edges, depth <= 2, bounded tokens, RRF k=60, agent attribution, configuration gating, and relation-poisoning defense.
- acceptance: Extended-fixture CI measurement is delivered; depth and budget invariants pass; relation poisoning passes before activation; active results expose traversal provenance; a losing leg ships dark and documented.
- scope: P-MEM-KG knowledge-graph retrieval

## REQ-p-xproj-opt-in
- source: .planning/sources/NANO-PROGRAM-PLAN.md
- description: Built last, restore cross-project reads only as explicit, logged, per-query, profile-tightenable opt-in behavior.
- acceptance: Each opt-in records scope, caller, and reason; default behavior remains bit-identical and security cards pass; profiles can disable opt-in entirely.
- scope: P-XPROJ cross-project reads
