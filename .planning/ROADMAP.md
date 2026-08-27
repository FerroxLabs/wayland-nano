# Roadmap: v1.1 Persistent Agent Program

## v1.1 Persistent Agent Program

## Overview

Thirteen new-milestone phases deliver all sixteen governing packages. Each phase is one owner/user-selected active goal and promotion PR. Sequential promotion is operating policy, not an invented dependency; technical eligibility follows the governing DAG.

## Governing Dependency DAG

- WP-0 → P-MEM-1.
- P-MEM-1 → P-MEM-SEC and P-PROF; WP-0 → P-MOD-GAP. These three first-wave lanes may run concurrently and close behind one gate.
- P-PROF + P-MOD-GAP → P-BOT-5a; P-BOT-5a + P-MEM-1 → P-BOT-5b; P-BOT-5b → P-BOT-5c.
- P-BOT-5a → P-EXE-1 → P-EXE-2. EXE-1 is eligible after Phase 3 and may parallel the 5b/5c workstream.
- P-MEM-1 → P-CONS and P-PROC; both are eligible after Phase 1 as independent workstreams.
- WP-0 → P-GRAPH-1 → P-GRAPH-2 → P-MEM-KG; P-MEM-KG also requires P-MEM-1 and the extended fixture. GRAPH-1 is eligible after Phase 1.
- P-MEM-1 + P-MEM-SEC → P-XPROJ, deliberately promoted last.

## Phases

- [ ] **Phase 1: Foundation Acceptance** - Review, merge, and freshly verify signed-contract PR #8.
- [ ] **Phase 2: Safe Composition Substrate** - Complete memory security and signed profile/module amendments in parallel lanes.
- [ ] **Phase 3: Named Agent Composition** - Define, resolve, spawn, and audit named agents.
- [ ] **Phase 4: Memory-Primary Persistent Agents** - Wire one shared scoped-memory activation seam across hosts.
- [ ] **Phase 5: Bounded Proactive Agents** - Add routines, escalation, retry, retention, and pause.
- [ ] **Phase 6: Hardened Browser Execution** - Dispatch named-agent browser actions through the hardened backend seam.
- [ ] **Phase 7: Hardened Desktop Execution** - Dispatch AX-first desktop actions through the same seam.
- [ ] **Phase 8: Memory-Aware Compaction** - Mediate compacted spans into scoped facts and decisions.
- [ ] **Phase 9: Procedure Extraction** - Mediate repeated successful work into retrievable procedures.
- [ ] **Phase 10: Code-Structure Index** - Replace regex repomap with measured tree-sitter structure.
- [ ] **Phase 11: Suggested Blast Radius** - Add measured-confidence Rust blast-radius suggestions.
- [ ] **Phase 12: KG Retrieval Leg** - Fixture-gate the bounded third RRF leg.
- [ ] **Phase 13: Explicit Cross-Project Reads** - Add host-authorized per-query reads last.

## Phase Details

### Phase 1: Foundation Acceptance
**Goal**: Signed-contract P-MEM-1 becomes a human-approved, merged, freshly verified foundation.
**Included packages**: WP-0, P-MEM-1
**Depends on**: Nothing; MEMORY-CONTRACT v1.2 was signed 2026-08-25
**Requirements**: REQ-wp-0-contract-freeze, REQ-p-mem-1-core-memory-store
**Success Criteria**:
1. Human review verifies `gates/**` and `agents/**` protection and approves open PR #8 without agent self-merge.
2. The merge retains recall@10 ≥0.90, zero project/agent leakage, query-equivalent kill recovery including `agent_id`, mediation proof, and seven green CI legs.
3. A fresh checkout of the merge SHA passes applicable workspace gates and exposes the contracted API/schema.
**Acceptance evidence**: Signed contract header/§13; PR review/merge SHA/CI; recall, rebuild, mediation, and fresh-checkout receipts.
**Anti-scope / tripwire**: Acceptance only; no rewrite, KG, hosted embeddings, compaction, profile, or agent-tool work. Any v1.1 memory baseline stops.
**Promotion gate**: Human-reviewed merge plus fresh-checkout green evidence.
**Plans**: TBD

### Phase 2: Safe Composition Substrate
**Goal**: Memory isolation, profile narrowing, and installed-module provenance are jointly ready for named agents.
**Included packages**: P-MEM-SEC, P-PROF, P-MOD-GAP
**Depends on**: P-MEM-1 for MEM-SEC/PROF; WP-0 for MOD-GAP
**Requirements**: REQ-p-mem-sec-gate-pack, REQ-p-prof-profiles, REQ-p-mod-gap-manifest-integrity
**Parallel lanes**: A—MEM-SEC; B—PROF; C—MOD-GAP; all close behind one completion gate.
**Success Criteria**:
1. Committed fixtures cover MEM-SEC-1 poisoned supersession, MEM-SEC-2 same-tier control, MEM-SEC-3 cross-project leak, MEM-SEC-4 extraction laundering/attribution, MEM-SEC-5 removed-scope escape, and MEM-SEC-6 cross-agent leak.
2. Applicable cards assert pass-1 FTS, pass-2 KNN, and final assembled output and receive `gates/**` owner review.
3. Before PROF implementation, owner signs version-stamped PROFILES-CONTRACT v1.1 with overlay-only kernel-core semantics; profile merge, selection, journaling, resume-narrows, and shipped profiles then pass.
4. Before MOD-GAP implementation, owner signs version-stamped NANO-MODULE-CONTRACT v1.1 covering `contract_version`, digest verification, receipt/provenance, and typed registry refusal; tampering then fails closed.
**Acceptance evidence**: Two signed amendments; six cards/checkpoints; profile and module batteries; one seven-leg integration CI.
**Anti-scope / tripwire**: Never weaken cards/checkpoints; no profile-v2 fields, registry governance, auto-fetch, hidden installers, or ABI. PROF ≤3 and MOD-GAP ≤2 days.
**Promotion gate**: Three lanes, two amendments, human review, local gates, and seven-leg CI green.
**Plans**: TBD

### Phase 3: Named Agent Composition
**Goal**: Hosts can define, resolve, spawn, and audit named agents under narrowed policy.
**Included packages**: P-BOT-5a
**Depends on**: P-PROF and P-MOD-GAP
**Requirements**: REQ-p-bot-5a-agent-composition
**Success Criteria**:
1. Valid agents scaffold/activate; malformed, untrusted, mismatched, reserved, unknown, or retired identities fail closed and IDs never recycle.
2. Spawn journals identity, composition hash, ceiling, persona, verified module digests, backend, and usage.
3. Every module ref resolves through verified installed provenance plus receipt; unknown, tampered, unreceipted, or refused-registry modules block activation.
4. Ceiling/prompt tests prevent widening/core replacement; assistant/reviewer/researcher work through host roster primitives.
**Acceptance evidence**: Identity tests; composition journal; valid receipt and four refusal classes; lattice/prompt tests; dogfood; `agents/**` review; seven-leg CI.
**Anti-scope / tripwire**: ≤5 days; no resume, memory, routines, Desktop roster UX, group chat, or second config format.
**Promotion gate**: Named activation and identity/module/ceiling negative evidence green locally and in CI.
**Plans**: TBD

### Phase 4: Memory-Primary Persistent Agents
**Goal**: Actual interactive hosts use one shared memory-primary runtime seam with scoped accumulation and visible receipts.
**Included packages**: P-BOT-5b
**Depends on**: P-BOT-5a and P-MEM-1
**Requirements**: REQ-p-bot-5b-persistence
**Success Criteria**:
1. Shared bootstrap performs identity/current posture → open P-MEM → scoped recall → fresh context assembly → expose recall/mediated propose → surface receipt.
2. CLI and at least one protocol adapter complete repeated activations; every interactive adapter demonstrably delegates to the same engine seam.
3. Identity/composition mismatch refuses unless rekeyed, revoked posture stays revoked, and contention returns `AgentBusy`.
4. MEM-SEC-6 stays green; ledger is parent-journal reconstructible, rollback is surgical, and GC exports before prune.
**Acceptance evidence**: CLI/protocol E2E chain; delegation, rekey/revocation/concurrency, receipt, security, ledger/rollback/GC tests; seven-leg CI.
**Anti-scope / tripwire**: ≤5 days; transcript replay is audit/fallback only; no duplicate host bootstrap, one-shot migration, Global reads, or routines.
**Promotion gate**: Shared-chain E2E across CLI/protocol plus security/rollback and seven-leg CI.
**Plans**: TBD

### Phase 5: Bounded Proactive Agents
**Goal**: Persistent agents run scheduled work within enforceable cost, retry, retention, escalation, and pause bounds.
**Included packages**: P-BOT-5c
**Depends on**: P-BOT-5b
**Requirements**: REQ-p-bot-5c-proactivity
**Success Criteria**:
1. Agent routines activate memory-primary with namespaced immutable prompt snapshots and narrowed mode.
2. Runs record cost, denials, result and bounded retention; typed failures enforce one compaction retry.
3. Attention is journaled/rate-limited under spin and pause blocks new activations until resume.
**Acceptance evidence**: Routine/history/cost/retention/failure/retry/spin/pause tests; seven-leg CI.
**Anti-scope / tripwire**: ≤4 days; no webhook platform, teach-by-demonstration, self-scheduling language, or Desktop UX.
**Promotion gate**: Routine, escalation, retry, retention, pause, and seven-leg CI green.
**Plans**: TBD

### Phase 6: Hardened Browser Execution
**Goal**: A named agent completes a browser action through the full hardened provenance chain.
**Included packages**: P-EXE-1
**Depends on**: P-BOT-5a; eligible after Phase 3 and parallel-capable with 5b/5c
**Requirements**: REQ-p-exe-1-browser-backend
**Success Criteria**:
1. Dispatch is resolved backend → capability intersection → ToolExecutor/CUA driver → supervisor/container → result → frame receipt → journal/model observation.
2. A named agent performs a browser action and replays its historical frame; captured/model vision tokens are metered.
3. Missing capability denies before dispatch; supervisor accepts only ensure/stop/reset/list and agent-derived names.
4. Image provenance, inspect hardening, CDP backpressure, size refusal, leases, and human-control refusal fail closed.
**Acceptance evidence**: Browser E2E; denial; historical frame; supervisor/tamper/image/lease/control tests; metering; seven-leg CI.
**Anti-scope / tripwire**: ≤5 days; no policy language, remote/cloud, desktop flavor, or caller-defined supervisor names.
**Promotion gate**: Complete dispatch and negative-path evidence plus seven-leg CI.
**Plans**: TBD

### Phase 7: Hardened Desktop Execution
**Goal**: A named agent completes an AX-first desktop action through the same seam while human control remains authoritative.
**Included packages**: P-EXE-2
**Depends on**: P-EXE-1
**Requirements**: REQ-p-exe-2-desktop-backend
**Success Criteria**:
1. The same dispatch chain selects desktop CUA/container and returns result, frame, journal, and model observation.
2. Named-agent AX-first action, pre-dispatch capability denial, and historical-frame proof pass.
3. Stopped desktops recreate rather than resume, who-is-driving pauses mid-turn, and shared hardening passes Docker and Podman.
**Acceptance evidence**: Desktop E2E/denial/frame/recreate/control plus both-runtime inspection and seven-leg CI.
**Anti-scope / tripwire**: ≤4 days; no host-control, remote VM, or second architecture.
**Promotion gate**: Desktop/shared negative evidence green on both runtimes and seven-leg CI.
**Plans**: TBD

### Phase 8: Memory-Aware Compaction
**Goal**: Compacted spans preserve useful scoped facts and decisions through mediated extraction.
**Included packages**: P-CONS
**Depends on**: P-MEM-1; eligible after Phase 1
**Requirements**: REQ-p-cons-memory-compaction
**Success Criteria**:
1. Extraction is host-mediated at ModelInference with visible receipts.
2. Rows remain same-scope retrievable after compaction and retention caps hold.
3. Measured extraction cost resolves the model choice.
**Acceptance evidence**: Roundtrip, trust/receipt/retention, cost report, mem-sec rerun, seven-leg CI.
**Anti-scope / tripwire**: ≤4 days; no tier elevation, hot-path LLM ingest, procedures, or routines.
**Promotion gate**: Extraction, retention, cost, isolation, and seven-leg CI green.
**Plans**: TBD

### Phase 9: Procedure Extraction
**Goal**: Repeated successful work becomes scoped retrievable knowledge without auto-execution.
**Included packages**: P-PROC
**Depends on**: P-MEM-1; eligible after Phase 1
**Requirements**: REQ-p-proc-procedure-extraction
**Success Criteria**:
1. Repeated-success fixture yields a proposal and only host mediation commits it at ModelInference with receipt.
2. Same-scope retrieval succeeds, other scopes see nothing, and receiptless procedures cannot land.
**Acceptance evidence**: Proposal/commit/receipt and positive/negative retrieval tests; seven-leg CI.
**Anti-scope / tripwire**: ≤3 days; no auto-execution, routine automation, or teach-by-demonstration.
**Promotion gate**: Proposal-to-retrieval/isolation evidence and seven-leg CI green.
**Plans**: TBD

### Phase 10: Code-Structure Index
**Goal**: Agents receive measured tree-sitter structure ranked into bounded context.
**Included packages**: P-GRAPH-1
**Depends on**: WP-0; eligible after Phase 1
**Requirements**: REQ-p-graph-1-code-index
**Success Criteria**:
1. Tracked-file def/ref index replaces regex internals and reports a committed localization benchmark.
2. Token ranking, edit churn, cache invalidation, and RSS limits pass.
**Acceptance evidence**: Benchmark and budget/churn/cache/RSS tests; seven-leg CI.
**Anti-scope / tripwire**: ≤6 days; no call edges, blast radius, LSP/SCIP, dynamic graph, or memory KG.
**Promotion gate**: Benchmark/correctness/resource evidence and seven-leg CI green.
**Plans**: TBD

### Phase 11: Suggested Blast Radius
**Goal**: Rust users receive heuristic blast-radius suggestions labeled by measured confidence.
**Included packages**: P-GRAPH-2
**Depends on**: P-GRAPH-1
**Requirements**: REQ-p-graph-2-blast-radius
**Success Criteria**:
1. Committed precision evaluation derives every displayed confidence and no surface asserts fact.
2. A losing evaluation ships dark with negative evidence preserved.
**Acceptance evidence**: Precision fixture/report, label/assertion tests, dark activation, seven-leg CI.
**Anti-scope / tripwire**: ≤3 days; no dynamic languages, unlabeled output, or asserted truth.
**Promotion gate**: Measured label/dark decision, presentation invariants, and seven-leg CI green.
**Plans**: TBD

### Phase 12: KG Retrieval Leg
**Goal**: Memory gains a secure bounded graph leg only when extended-fixture evidence supports activation.
**Included packages**: P-MEM-KG
**Depends on**: P-MEM-1, extended fixture, and graph ordering after P-GRAPH-2
**Requirements**: REQ-p-mem-kg-retrieval
**Success Criteria**:
1. Attributed nodes/edges feed BFS depth ≤2 under token budget and RRF k=60.
2. Relation poisoning passes before activation and traversal provenance is visible.
3. Extended recall records winning activation or documented dark result without weakening FTS/KNN.
**Acceptance evidence**: Fixture measurement, invariants, poisoning, provenance, active/dark config, seven-leg CI.
**Anti-scope / tripwire**: ≤3 days; no depth >2, community detection, LLM entity resolution, or activation without a win.
**Promotion gate**: Measurement/poisoning/invariants/decision and seven-leg CI green.
**Plans**: TBD

### Phase 13: Explicit Cross-Project Reads
**Goal**: Authorized callers make one auditable cross-project query while isolation remains default.
**Included packages**: P-XPROJ
**Depends on**: P-MEM-1 and P-MEM-SEC; promoted last by policy
**Requirements**: REQ-p-xproj-opt-in
**Success Criteria**:
1. Host issues a per-query authorization decision/token binding caller, scope, and reason; use is journaled.
2. Model args cannot self-authorize `Global`; missing/invalid/stale/mismatched authorization fails before retrieval.
3. Default behavior is bit-identical, profiles can disable opt-in, and cross-project never implies cross-agent.
**Acceptance evidence**: Authorization trace/negative tests, default comparison, profile/resume narrowing, mem-sec rerun, seven-leg CI.
**Anti-scope / tripwire**: ≤2 days; no sticky/config Global, implicit widening, cross-agent access, or convenience default.
**Promotion gate**: Host authorization, fail-closed/default-isolation, and seven-leg CI green.
**Plans**: TBD

## Requirement Coverage

| Phase | Packages | Requirements | Count |
|---:|---|---|---:|
| 1 | WP-0, P-MEM-1 | REQ-wp-0-contract-freeze, REQ-p-mem-1-core-memory-store | 2 |
| 2 | P-MEM-SEC, P-PROF, P-MOD-GAP | REQ-p-mem-sec-gate-pack, REQ-p-prof-profiles, REQ-p-mod-gap-manifest-integrity | 3 |
| 3 | P-BOT-5a | REQ-p-bot-5a-agent-composition | 1 |
| 4 | P-BOT-5b | REQ-p-bot-5b-persistence | 1 |
| 5 | P-BOT-5c | REQ-p-bot-5c-proactivity | 1 |
| 6 | P-EXE-1 | REQ-p-exe-1-browser-backend | 1 |
| 7 | P-EXE-2 | REQ-p-exe-2-desktop-backend | 1 |
| 8 | P-CONS | REQ-p-cons-memory-compaction | 1 |
| 9 | P-PROC | REQ-p-proc-procedure-extraction | 1 |
| 10 | P-GRAPH-1 | REQ-p-graph-1-code-index | 1 |
| 11 | P-GRAPH-2 | REQ-p-graph-2-blast-radius | 1 |
| 12 | P-MEM-KG | REQ-p-mem-kg-retrieval | 1 |
| 13 | P-XPROJ | REQ-p-xproj-opt-in | 1 |
| **Total** | **16 packages** | **16/16 exactly once** | **16** |

## Progress

**One-active-goal promotion order:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13. Eligibility follows the DAG.

| Phase | Plans Complete | Status | Completed |
|---|---:|---|---|
| 1. Foundation Acceptance | 0/TBD | Awaiting PR review/merge | - |
| 2. Safe Composition Substrate | 0/TBD | Not started | - |
| 3. Named Agent Composition | 0/TBD | Not started | - |
| 4. Memory-Primary Persistent Agents | 0/TBD | Not started | - |
| 5. Bounded Proactive Agents | 0/TBD | Not started | - |
| 6. Hardened Browser Execution | 0/TBD | Not started | - |
| 7. Hardened Desktop Execution | 0/TBD | Not started | - |
| 8. Memory-Aware Compaction | 0/TBD | Not started | - |
| 9. Procedure Extraction | 0/TBD | Not started | - |
| 10. Code-Structure Index | 0/TBD | Not started | - |
| 11. Suggested Blast Radius | 0/TBD | Not started | - |
| 12. KG Retrieval Leg | 0/TBD | Not started | - |
| 13. Explicit Cross-Project Reads | 0/TBD | Not started | - |
