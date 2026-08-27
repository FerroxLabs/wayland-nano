# Requirements: v1.1 Workable Persistent Agent

**Defined:** 2026-08-27
**Coverage rule:** Every active requirement maps to exactly one phase.

## Active Requirements

- [ ] **REQ-FOUND-01** — Human-review/merge PR #8 and freshly verify WP-0/P-MEM-1 evidence without reopening scope.
- [ ] **REQ-ARCH-01** — Owner-sign a manifest listing every governing artifact/version/signature/disposition/precedence and pinning ownership, Nano security enrollment, trusted issuer/key lifecycle, immutable binding, alias compatibility, protected fixture ownership, merge order, and compatibility window.
- [ ] **REQ-ACT-01** — Accept a minimal authenticated descriptor containing principal/project, optional audit product ref, activation/session/idempotency IDs, continuity, capabilities, budgets/deadline, replay protection, and a resume-context fingerprint limited to policy/tool/persona/module refs needed for resume safety.
- [ ] **REQ-POL-01** — Verify the trusted issuer and intersect Nano ceilings; freeze canonical receipts; direct CLI uses an enrolled local issuer and explicit `main` compatibility mapping, never an identity bypass; tamper, replay, revoked issuer, remap, substitution, widening, and unauthorized controls fail typed.
- [ ] **REQ-MEM-01** — Complete MEM-SEC-1–6 across FTS, KNN, and final assembly using `(project, principal_id)` semantics mapped 1:1 to existing physical `agent_id`.
- [ ] **REQ-MEM-02** — Wire `nano-memory` into the actual shared CLI and ACP activation path; preserve old DB/journal rebuild and query equivalence including `agent_id` and receipts, with no silent filesystem-memory bypass.
- [ ] **REQ-CONT-01** — Measure `fresh`, `session_resume`, and `memory_recall`; publish a recommendation while Desktop remains the default-setting authority.
- [ ] **REQ-RUN-01** — Execute idempotent Desktop-triggered activations with immutable request binding, metering, deadline, cancel/pause, bounded retry/retention, escalation and emergency-refusal receipts; Nano stores no schedule or timer.
- [ ] **REQ-DOG-01** — Immediately dogfood repeated interactive and scheduled activations and issue an evidence-based accept/revise/reject decision for the boundary and next milestone.

## Deferred Program Inventory

| Scope | Disposition |
|---|---|
| P-PROF, P-BOT-5a product registry/composition | Desktop-owned; not Nano work. |
| P-MOD-GAP composition digests | Add only when a concrete Nano enforcement consumer exists. |
| P-BOT-5b | Reframed as runtime wiring and measured continuity in Phase 3. |
| P-BOT-5c | Split: Desktop schedules; Nano bounded execution in Phase 4. |
| P-EXE-1 browser | Separate v1.2 milestone. |
| P-EXE-2 desktop | Separate v1.3 milestone after browser provider. |
| P-CONS, P-PROC | Evidence-gated future milestones. |
| P-GRAPH-1/2, P-MEM-KG | Later code-intelligence program. |
| P-XPROJ | Later security milestone. |

## Definition of Done

Each phase meets observable criteria, passes governing local and CI gates, and ends in reproducible evidence or exact handoff. Three strikes and anti-scope tripwires stop the phase.

## Traceability

| Requirement | Phase | Status |
|---|---:|---|
| REQ-FOUND-01 | 1 | PR #8 merge/fresh verification pending |
| REQ-ARCH-01 | 1 | Owner ratification pending |
| REQ-ACT-01 | 2 | Pending ratification |
| REQ-POL-01 | 2 | Pending ratification |
| REQ-MEM-01 | 3 | Pending ratification |
| REQ-MEM-02 | 3 | Pending ratification |
| REQ-CONT-01 | 3 | Pending ratification |
| REQ-RUN-01 | 4 | Pending ratification |
| REQ-DOG-01 | 4 | Pending ratification |

**Coverage:** 9/9 exactly once.
