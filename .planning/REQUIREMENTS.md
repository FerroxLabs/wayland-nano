# Requirements: v1.1 Workable Persistent Agent

**Defined:** 2026-08-27
**Coverage rule:** Every active requirement maps to exactly one phase.

## Active Requirements

- [x] **REQ-FOUND-01** — Merge PR #8 through the owner-directed, agent-operated TradeCanyon account workflow with exact attribution, then freshly verify WP-0/P-MEM-1 evidence without reopening scope.
- [x] **REQ-ARCH-01** — Owner-sign a manifest listing every governing artifact/version/signature/disposition/precedence and pinning ownership, Nano security enrollment, trusted issuer/key lifecycle, immutable binding, alias compatibility, protected fixture ownership, merge order, and compatibility window.
- [x] **REQ-ACT-01** — Route both Desktop ACP stacks through one shared Nano admission gate and accept a minimal authenticated descriptor containing mandatory opaque product subject, principal/project, activation/session/idempotency IDs, continuity, capabilities, budgets/deadline, replay protection, and a resume-context fingerprint limited to policy/tool/persona/module refs needed for resume safety.
- [x] **REQ-POL-01** — Verify the trusted issuer and local project grant, intersect Nano ceilings, freeze canonical receipts, quarantine all unauthenticated filesystem/T2-memory and cron/routine persistence before enablement, and require direct CLI to use an enrolled local issuer and explicit `main` mapping; tamper, replay, revoked issuer, remap, substitution, widening, bypass, and unauthorized controls fail typed.
- [ ] **REQ-MEM-01** — Complete MEM-SEC-1–6 across FTS, KNN, and final assembly using `(project, principal_id)` semantics mapped 1:1 to existing physical `agent_id`.
- [ ] **REQ-MEM-02** — After Phase 2 quarantine, migrate/replace the old filesystem-memory path and wire `nano-memory` into the shared CLI and both ACP activation paths; preserve old DB/journal rebuild and query equivalence including `agent_id` and receipts.
- [ ] **REQ-CONT-01** — Measure `fresh`, `session_resume`, and `memory_recall`; publish a recommendation while Desktop remains the default-setting authority.
- [ ] **REQ-RUN-01** — Migrate/remove the Nano cron/routine surfaces already quarantined in Phase 2 and execute idempotent Desktop-triggered activations with immutable request binding, metering, deadline, cancel/pause, bounded retry/retention, escalation and emergency-refusal receipts; Nano stores no schedule or timer.
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
| REQ-FOUND-01 | 1 | Complete — PR #8 merged at `5bd5451`; full fresh eight-command receipt passed |
| REQ-ARCH-01 | 1 | Complete — signed amendment SHA `9107e3c7…`; strict authority/governance gates passed |
| REQ-ACT-01 | 2 | Complete — Nano PRs #12–#17 and Desktop PRs #1277–#1279 merged; dual-stack/CLI shared-gate matrix green; independent fresh-checkout rerun exit 0 (02-VERIFICATION.md rows 1–3, 8–13, 17) |
| REQ-POL-01 | 2 | Complete — default-off enablement, quarantine, typed-refusal negative matrix (26 rows), protected review chain and frozen ceremony evidence verified (02-VERIFICATION.md rows 5–6, 12–18) |
| REQ-MEM-01 | 3 | Pending ratification |
| REQ-MEM-02 | 3 | Pending ratification |
| REQ-CONT-01 | 3 | Pending ratification |
| REQ-RUN-01 | 4 | Pending ratification |
| REQ-DOG-01 | 4 | Pending ratification |

**Coverage:** 9/9 exactly once.
