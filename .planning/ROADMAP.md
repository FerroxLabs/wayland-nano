# Roadmap: v1.1 Workable Persistent Agent

## Overview

Four phases deliver the smallest useful agent: Desktop supplies trusted identity and triggers; Nano supplies scoped continuity, bounds, and evidence. Browser and desktop execution are later milestones.

## Phases

- [ ] **Phase 1: Ownership Contract and Foundation** - Ratify exact authority/compatibility rules and accept PR #8.
- [ ] **Phase 2: Minimal Authenticated Activation** - Establish the smallest fail-closed Desktop→Nano request.
- [ ] **Phase 3: Runtime-Integrated Scoped Continuity** - Secure and wire real memory, then measure continuity.
- [ ] **Phase 4: Bounded Trigger and Immediate Dogfood** - Retire Nano scheduling, run Desktop-triggered work, and decide from evidence.

## Phase Details

### Phase 1: Ownership Contract and Foundation

**Goal**: A signed authority/compatibility contract and freshly verified P-MEM-1 foundation make later work unambiguous.
**Depends on**: Nothing
**Requirements**: REQ-FOUND-01, REQ-ARCH-01
**Success Criteria** (what must be TRUE):

1. PR #8 is human-merged and fresh-checkout evidence retains recall@10 ≥0.90, zero project/agent leakage, kill rebuild/query equivalence, mediation, and seven CI legs.
2. A signed manifest enumerates every source artifact/version/signature/disposition/precedence; Desktop owns product binding while Nano keeps only security enrollment needed for issuer and anti-remap enforcement.
3. The amendment pins issuer/trust root, provisioning/rotation/revocation, immutable never-remap/reassignment policy and negative cases, plus wire `principal_id` as a 1:1 semantic alias of physical/schema/journal `agent_id` for v1.1.
4. It names the protected MEM-SEC fixture amendment/owner, authoritative Desktop seam (`AcpConnection`), carrier/version/downgrade rules, merge order, exact Nano artifact-SHA test, and compatibility exit criteria.

**Acceptance evidence**: Merge SHA/CI/fresh receipts; signed versioned contracts; owned fixture and compatibility matrix.
**Anti-scope / tripwire**: Only foundation acceptance and contract ratification. Ambiguity, unsigned authority, or unowned fixture stops and re-roadmaps.
**Plans**: 4 plans

Plans:
**Wave 1**

- [ ] 01-01-PLAN.md — Prepare the hash-backed owner authority amendment.
- [ ] 01-02-PLAN.md — Freeze PR #8 pre-merge and focused local evidence.

**Wave 2** *(blocked on Wave 1 completion)*

- [ ] 01-03-PLAN.md — Obtain owner signature and human PR merge.

**Wave 3** *(blocked on Wave 2 completion)*

- [ ] 01-04-PLAN.md — Freshly verify merged master and close Phase 1.

### Phase 2: Minimal Authenticated Activation

**Goal**: Nano accepts only a minimal trusted-issuer assertion and independently narrows it before activation.
**Depends on**: Phase 1
**Requirements**: REQ-ACT-01, REQ-POL-01
**Success Criteria** (what must be TRUE):

1. The versioned carrier contains principal/project, optional audit product ref, activation/session/idempotency, continuity, capabilities, budgets/deadline, replay protection, and a resume fingerprint limited to policy/tool/persona/module refs required to detect unsafe resume drift.
2. Nano verifies issuer, freshness/replay and local ceilings; canonical activation/control receipts have frozen vocabulary, correlation fields and offline verification.
3. Tamper, downgrade, replay, revoked issuer, remap, substitution, widening, unauthorized cancel/pause, and cancel/complete races follow pinned ordering and typed receipts; fingerprint drift fails closed or explicitly falls back to a fresh activation.
4. Direct CLI uses a locally enrolled issuer and explicit `main` compatibility principal under the same trust/identity rules; the cross-repo fixture runs through `AcpConnection` against the exact Nano artifact SHA.

**Acceptance evidence**: Fixture/schema vectors, positive receipt, negative matrix, exact-artifact cross-repo run and both CI systems.
**Anti-scope / tripwire**: Stop, hand off, and replan if descriptor, receipts, or enrollment cannot land as bounded subdeliverables or grow a product registry, scheduler, unused composition framework, policy language, or Desktop edits here.
**Plans**: TBD

### Phase 3: Runtime-Integrated Scoped Continuity

**Goal**: Real CLI and ACP activations use one secure scoped-memory seam with measured continuity choices.
**Depends on**: Phase 2
**Requirements**: REQ-MEM-01, REQ-MEM-02, REQ-CONT-01
**Success Criteria** (what must be TRUE):

1. MEM-SEC-1–6 pass at FTS, KNN, and final assembly under `(project, principal_id)` wire semantics with zero leakage and attribution preserved.
2. CLI and authoritative Desktop `AcpConnection` activations traverse the same `nano-memory` seam; filesystem memory is migrated/disabled/subordinate and cannot bypass scope.
3. Existing physical `agent_id` remains 1:1 with wire `principal_id`; old DB and journal rebuild produce query-equivalent rows and receipts including `agent_id`.
4. Continuity modes are measured for quality/latency/tokens; `session_resume` fails closed on composition/policy drift, the report recommends defaults, and Desktop selects them.

**Acceptance evidence**: Six security cards, shared-seam E2E, legacy rebuild/equivalence and bypass negatives, strategy benchmark, exact-artifact cross-repo run, seven CI legs.
**Anti-scope / tripwire**: Stop, hand off, and replan if security, compatibility rebuild, runtime seam, or evaluation cannot land independently. No schema rename, hosted memory, MCP expansion, extraction, graph/KG, global reads, or duplicate bootstrap.
**Plans**: TBD

### Phase 4: Bounded Trigger and Immediate Dogfood

**Goal**: Desktop alone fires repeated bounded Nano work and the end-to-end evidence decides whether the architecture is workable.
**Depends on**: Phase 3
**Requirements**: REQ-RUN-01, REQ-DOG-01
**Success Criteria** (what must be TRUE):

1. Existing Nano cron/scheduling authority is inventoried and migrated or disabled for Desktop-controlled activations; Nano timer/tool advertisement is removed/disabled and a negative test proves Desktop is the sole firing authority.
2. Desktop derives a stable job-occurrence idempotency key and triggers through `AcpConnection`; Nano deduplicates activation admission, journal/memory/receipt commit, and effect-dispatch ID.
3. External effects use an intent ledger and return typed `unknown_outcome` plus manual reconciliation after ambiguous failure unless the provider proves an idempotent commit; no false exact-once claim is made.
4. Nano enforces request binding, cost/deadline, cancel/pause, retry/retention, escalation and emergency refusal while storing no schedules/timers.
5. Repeated interactive/scheduled dogfood proves scoped recall, denial/replay/remap resistance, cancellation, crash ambiguity handling, and issues an accept/revise/reject owner decision plus next-milestone choice.

**Acceptance evidence**: Scheduler inventory/migration; sole-fire negative; occurrence derivation/dedup and intent-ledger tests; dogfood receipts/report; exact Nano artifact-SHA Desktop run and governing CI.
**Anti-scope / tripwire**: Stop, hand off, and replan if scheduler retirement, bounded execution, ambiguity ledger, or dogfood cannot land independently. No provider, Nano timer/schedule, UI, unbounded retry, extraction, graph/KG, or cross-project work.
**Plans**: TBD

## Coverage

| Phase | Requirements |
|---:|---|
| 1 | REQ-FOUND-01, REQ-ARCH-01 |
| 2 | REQ-ACT-01, REQ-POL-01 |
| 3 | REQ-MEM-01, REQ-MEM-02, REQ-CONT-01 |
| 4 | REQ-RUN-01, REQ-DOG-01 |

**Coverage:** 9/9 exactly once.

## Implementation Ownership and Regression

| Invariant | Implemented in | Revalidated in |
|---|---|---|
| Source/issuer/identity compatibility | Phase 1 | Phases 2–4 |
| Descriptor, receipts, replay and controls | Phase 2 | Phases 3–4 |
| Scope isolation, rebuild and continuity | Phase 3 | Phase 4 |
| Scheduler retirement, idempotency and effect ambiguity | Phase 4 | Phase 4 dogfood |

## Subsequent Focused Milestones

- **v1.2 Hardened Browser Provider:** activation-bound sandbox; explicit negative gate proving ordinary Playwright MCP cannot bypass Nano policy/provenance; dedicated Docker/Podman capability, inspect-hardening, lease, teardown, frame, and denial matrix.
- **v1.3 Hardened Desktop Provider:** AX-first provider on the same contract; explicit negative gate proving host CUA cannot bypass Nano policy/provenance; dedicated Docker/Podman recreate, human-control, lease, frame, and denial matrix.
- Later evidence-gated milestones: compaction/procedure extraction, code graph/blast/KG, cross-project reads.

## Progress

| Phase | Plans Complete | Status | Completed |
|---|---:|---|---|
| 1. Ownership Contract and Foundation | 0/TBD | Awaiting ratification/merge | - |
| 2. Minimal Authenticated Activation | 0/TBD | Not started | - |
| 3. Runtime-Integrated Scoped Continuity | 0/TBD | Not started | - |
| 4. Bounded Trigger and Immediate Dogfood | 0/TBD | Not started | - |
