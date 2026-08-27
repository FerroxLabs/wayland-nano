# Requirements: v1.1 Persistent Agent Program

**Defined:** 2026-08-27
**Core Value:** A change earns trust only through independently rerunnable machine evidence.
**Authority:** NANO-PROGRAM-PLAN plus signed MEMORY-CONTRACT v1.2, PROFILES-CONTRACT, and NANO-MODULE-CONTRACT.
**Coverage rule:** Every current-milestone requirement maps to exactly one phase.

## Current Milestone Requirements

- [ ] **REQ-wp-0-contract-freeze** — Preserve signed MEMORY-CONTRACT v1.2 (2026-08-25) and verify human-review protection for `gates/**` and `agents/**`; PR #8 review, merge, and fresh-checkout evidence remain. *(WP-0)*
- [ ] **REQ-p-mem-1-core-memory-store** — Land journal-authoritative SQLite/FTS5/sqlite-vec memory with scoped bi-temporal content, deterministic trust resolution, FTS+KNN RRF, mediation, retention, and unpopulated KG schema; prove recall, isolation, kill recovery, mediation, and seven-leg CI. *(P-MEM-1)*
- [ ] **REQ-p-mem-sec-gate-pack** — Commit fixture rows for MEM-SEC-1 poisoned supersession, MEM-SEC-2 same-tier control, MEM-SEC-3 cross-project leak, MEM-SEC-4 extraction laundering/attribution, MEM-SEC-5 removed-scope escape, and MEM-SEC-6 cross-agent leak; assert pass-1 FTS, pass-2 KNN, and final output. *(P-MEM-SEC)*
- [ ] **REQ-p-prof-profiles** — Owner-review/sign a version-stamped PROFILES-CONTRACT v1.1 overlay-only amendment before implementing closed profiles, narrow-only merge/selection, `Op::ProfileSet`, resume-narrows, and three built-ins. *(P-PROF)*
- [ ] **REQ-p-mod-gap-manifest-integrity** — Owner-review/sign a version-stamped NANO-MODULE-CONTRACT v1.1 amendment for `contract_version`, digest pins/verification, receipts/provenance, and typed registry refusal before implementation. *(P-MOD-GAP)*
- [ ] **REQ-p-bot-5a-agent-composition** — Ship named registry/config, identity-bearing spawn/usage, journaled composition, ceiling/persona/roster/scaffold/dogfood; module refs require verified installed provenance and receipts or fail closed. *(P-BOT-5a)*
- [ ] **REQ-p-bot-5b-persistence** — Ship one shared memory-primary host seam: identity/current posture → open P-MEM → scoped recall → context → recall/mediated propose → receipt; enforce concurrency, ledger, rollback, and export-before-prune. *(P-BOT-5b)*
- [ ] **REQ-p-bot-5c-proactivity** — Ship bounded agent-addressed routines, immutable prompt receipts, metering/denials, rate-limited attention, typed failure/retry, retention, and pause. *(P-BOT-5c)*
- [ ] **REQ-p-exe-1-browser-backend** — Ship backend dispatch from resolution through capability intersection, executor/driver, supervisor/container, result, frame receipt, and journal/model observation; prove named-agent browser action, denial, provenance, lease, and historical frame. *(P-EXE-1)*
- [ ] **REQ-p-exe-2-desktop-backend** — Ship hardened XFCE/VNC on the same seam with recreate-not-resume, AX-first named-agent action, denial, historical frame, who-is-driving, and Docker/Podman proof. *(P-EXE-2)*
- [ ] **REQ-p-cons-memory-compaction** — Extract compacted facts/decisions through host mediation at ModelInference with receipts, retention, retrievability, and measured model-cost decision. *(P-CONS)*
- [ ] **REQ-p-proc-procedure-extraction** — Mine repeated successful journal shapes into proposed, mediated, scoped procedures with receipts and later same-scope retrieval; never auto-execute. *(P-PROC)*
- [ ] **REQ-p-graph-1-code-index** — Replace regex repomap with tree-sitter def/ref indexing, incremental/cache correctness, tracked-file scope, budget ranking, benchmark, and RSS limits. *(P-GRAPH-1)*
- [ ] **REQ-p-graph-2-blast-radius** — Add Rust-only suggested blast radius labeled by measured precision; ship dark with documented negative result if it loses. *(P-GRAPH-2)*
- [ ] **REQ-p-mem-kg-retrieval** — Build and measure bounded depth-2 KG-BFS RRF with attribution, provenance, config gate, and relation-poisoning defense; activate only if it wins. *(P-MEM-KG)*
- [ ] **REQ-p-xproj-opt-in** — Built last, add only host-mediated per-query cross-project authorization that models cannot self-authorize, journaled/profile-tightenable with isolated defaults intact. *(P-XPROJ)*

## Definition of Done

Each phase satisfies its package evidence without weakening policy or gates, passes local touched-crate checks and seven-leg CI, preserves required identity/scope/provenance/receipts, and ends in reproducible evidence or a continuation handoff.

## Traceability

| Requirement | Package | Phase | Status |
|---|---|---:|---|
| REQ-wp-0-contract-freeze | WP-0 | Phase 1 | Signed; awaiting PR review/merge |
| REQ-p-mem-1-core-memory-store | P-MEM-1 | Phase 1 | Implemented on open PR #8 |
| REQ-p-mem-sec-gate-pack | P-MEM-SEC | Phase 2 | Pending |
| REQ-p-prof-profiles | P-PROF | Phase 2 | Pending |
| REQ-p-mod-gap-manifest-integrity | P-MOD-GAP | Phase 2 | Pending |
| REQ-p-bot-5a-agent-composition | P-BOT-5a | Phase 3 | Pending |
| REQ-p-bot-5b-persistence | P-BOT-5b | Phase 4 | Pending |
| REQ-p-bot-5c-proactivity | P-BOT-5c | Phase 5 | Pending |
| REQ-p-exe-1-browser-backend | P-EXE-1 | Phase 6 | Pending |
| REQ-p-exe-2-desktop-backend | P-EXE-2 | Phase 7 | Pending |
| REQ-p-cons-memory-compaction | P-CONS | Phase 8 | Pending |
| REQ-p-proc-procedure-extraction | P-PROC | Phase 9 | Pending |
| REQ-p-graph-1-code-index | P-GRAPH-1 | Phase 10 | Pending |
| REQ-p-graph-2-blast-radius | P-GRAPH-2 | Phase 11 | Pending |
| REQ-p-mem-kg-retrieval | P-MEM-KG | Phase 12 | Pending |
| REQ-p-xproj-opt-in | P-XPROJ | Phase 13 | Pending |

**Coverage:** 16/16 current-milestone requirements mapped exactly once.
