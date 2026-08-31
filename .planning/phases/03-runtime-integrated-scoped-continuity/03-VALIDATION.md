# Phase 3 Nyquist Validation Strategy

## Principle

Every trust claim has a failing-before-implementation gate, an external oracle, and a final exact-artifact repetition. Source grep, mocks, runtime self-report and a green compiler are never sufficient. Each behavior below names its first owning plan and final closure in Plan 03-06, with independent verdict reserved to Plan 03-07.

Authority: 03-CONTEXT (D3-01..D3-10), 03-RESEARCH, signed MEMORY-CONTRACT v1.2 §3/§5/§6.1-6.9/§11.

## Per-Plan Contract / Harness Table

| Contract / harness | First owner | Required failing evidence before production code | Final oracle |
|---|---:|---|---|
| mem-sec card + sealed fixtures + registry entry | 01 | `verify --gate mem-sec --run-only` fails closed with the harness absent (missing subject FAILs, never skips) | 03-01: 7-leg CI green on the exact PR head; 03-06 merged-tree closure repeat; 03-07 fresh-checkout rerun |
| Six-card Rust harness (MS-01..MS-06) | 01 | Each check's >=5 fluent-but-wrong mutants are caught in detached exact-base worktrees | Mutant-caught evidence per check + `gate: 6/6` through the WP-3 entry |
| Pack meta-test (pool coverage, seals, closure digest) | 01 | Lands with the harness in commit 2 because it must fail when that subject is absent; dropped mutant below 5/check, corrupted seal, or stale digest turns it red | `node --test gates/tests/gates-mem-sec.test.cjs` on the PR head and merged tree |
| Host MemoryPolicy source | 02 | Global/unknown source_trust/agent_scope parse and disabled-by-default tests fail before the module exists | Typed parse errors + journaled Op::MemoryPolicyResolved on every persistent entrypoint |
| MemoryRecall admission arm | 02 | Typed negatives fail against the unconditional ContinuityNotEnabled arm | admission_matrix green with every fallback/drift misuse row; session_resume/fresh rows byte-unchanged |
| Journal topology decision | 03 | No code before the decision record exists with both options' consequences | 03-04 equivalence proofs executed under the recorded choice |
| The one memory seam (4 points) | 03 | Seam E2E fails before wiring: no scoped retrieval, no recall tools, no memory_propose under activation | activation_memory_seam green; 03-06 cross-repo exact-artifact regression |
| Write authority (§6.7 mediation) | 03 | Direct model-tier write attempts fail MediationRequired on every exposed surface | MEM-SEC-4 + seam negatives; user-visible journaled receipts |
| Extended quarantine | 03/04 | New negative rows fail while legacy routes or unauthenticated seams are reachable | activation_quarantine strictly larger, all Phase-2 rows unchanged, external-state oracle byte-identical |
| Legacy migration / disable-subordinate | 04 | Migration round-trip, tier-laundering, and authority-regain negatives fail before the ingestion path exists | memory_migration green + §11 bar 2 equivalence incl. agent_id |
| Old-DB vs rebuild equivalence | 04 | Equivalence test fails where agent_id/receipts diverge under the chosen topology | corrective_regressions extended rows green; MEM-SEC-4 green over migrated content |
| Continuity measurement harness | 05 | Harness refuses an unmarked binary; budget-hash mismatch refuses report rendering | Multi-seed receipt runs + published report; Desktop consumption noted in 03-06 |

## Requirement Test Matrix

| Requirement behavior | Focused command | Closure |
|---|---|---|
| REQ-MEM-01: MEM-SEC-1..6 at FTS, KNN, and final assembly, zero leakage, attribution preserved | `cargo test -p nano-memory --test mem_sec_cards -- --test-threads=1` then `target/debug/wayland-nano.exe verify --gate mem-sec --run-only` | 7-leg CI with mem-sec on every leg (03-06); verifier rerun from fresh merged checkout (03-07) |
| REQ-MEM-01: anti-self-grading + owner review | `node --test gates/tests/gates-mem-sec.test.cjs` | Two-commit boundary (fixtures/labels vs retrieval implementation) + §6.6 review record in the 03-06 receipt |
| REQ-MEM-02: one seam across CLI/ACP-stacks/protocol-host/exec | `cargo test -p nano-cli --test activation_memory_seam --test activation_admission -- --test-threads=1` | Exact-artifact cross-repo run reusing the Phase 2 Desktop verifier pipeline shape (03-06) |
| REQ-MEM-02: legacy migrated/disabled/subordinated, no authority regain | `cargo test -p nano-cli --test memory_migration --test activation_quarantine -- --test-threads=1` | Default-off preservation manifest with external oracles (03-06) |
| REQ-MEM-02: old-DB/rebuild query-equivalence incl. agent_id | `cargo test -p nano-memory --test corrective_regressions --test durability -- --test-threads=1` | §11 bar 2 rows verified from fresh checkout (03-07) |
| REQ-CONT-01: memory_recall admitted with pinned fallback; session_resume drift unchanged | `cargo test -p nano-activation --test admission_matrix -- --test-threads=1` | Admission regression rows in the cross-repo run (03-06) |
| REQ-CONT-01: measured modes + recommendation, Desktop selects | `node scripts/soak/continuity.mjs --mode receipt --seed <s>` + `node scripts/soak/continuity-report.mjs --evidence-dir scripts/soak/evidence --out docs/evidence/phase3/continuity-modes-report.md` | Report numbers traced to NDJSON manifests by the verifier (03-07); consumption evidence noted (03-06) |

## Security Mutation Matrix (memory-specific)

- Poisoned writes: lower-tier newer write against a higher-tier currently-valid fact (KeepExisting); same-tier 1.2x-confidence supersession fires; exact adjusted-confidence tie keeps existing; near-tie coexist stamped x0.8; conflict domain never crosses `(project, agent_id, subject, predicate)`.
- Tier laundering: LLM-extracted rows from ToolOutput/User episodes cap at ModelInference; direct `write_*` at ModelInference returns MediationRequired; replay/rebuild/export preserve tier and `agent_id` bit-for-bit; migration cannot grant User/ToolOutput tier to legacy .md content (ambiguous origin → ModelInference).
- Cross-project escape: query project B with 30/30 seeded split — zero project-A rows at pass-1 FTS, pass-2 KNN, AND fused output; partition predicates as WHERE clauses, never post-filters; any `ReadScope::Global` spelling in config or journal is a typed parse error.
- Cross-agent escape: bot-b query with default `agent_scope = Own` — zero bot-a rows at all three checkpoints; explicit-list agent_scope stays a typed refusal; session keys embed `agent:<id>:` and namespace collisions are typed hard errors.
- Removed-scope escape: unknown `source_trust`/`agent_scope`/`read_scope` values and unconfigured `agent_id` fail typed at parse, session start, and write; no silent coercion, no default-to-widest.
- Write-authority probes: model-originated commit attempted through recall tools, propose tool, and a direct store handle — only the §6.7 mediated path lands, force-capped, screened deterministically, journaled with agent_id/source_trust/project, receipted; no LLM screener exists to attack.
- Resume-drift: memory_recall with unpinned/self/widened fallback; session_resume with drifted issuer/subject/principal/project/session/fingerprint/epochs — all typed refusals; drift checks byte-unchanged from Phase 2.
- Migration authority-regain: re-migration idempotent-or-refused; hand-edited post-migration .md invisible to the seam; legacy tool names UnknownTool under activation; legacy `Op::MemoryWrite*`/`Cron*` replay as no-ops granting no retrieval authority; quarantined backstops intact.
- Seam/default-off: unauthenticated exec/ACP and policy-disabled sessions expose no recall/propose tools and no memory block; seeded memory/, t2/, cron/jobs.json, hooks.toml byte-identical after runs.
- Evidence anti-gaming: fixture labels unchanged (seal pinned); budgets frozen and hash-pinned before full runs; closure digest recompute; the four gate.yml inventory points asserted; manifests non-self-referential.

## Wave Gates

Plan 03-01 uses exactly two commits pushed together: commit 1 owns fixtures, labels,
card, script, registry, `gate.yml`, the gate-script LF pin, this plan, and this validation contract, with its applicable checks green and the
missing-harness refusal recorded as expected failing-first evidence. Commit 2 owns the
Rust harness/store/types, harness-dependent meta-test, and summary and is fully green.
This keeps fixture/label authorship separate from retrieval implementation without
committing a knowingly failing meta-test.

Owner override 2026-08-31: Plan 03-01 uses one protected PR. Its mandatory two-commit
boundary satisfies §11.1's normative separate commit/PR anti-self-grading rule for
this plan and supersedes the stale explanatory phrase "both PRs". Both commits remain
subject to the same `gates/**` owner-review path; no fixture/label bytes may change in
commit 2.

1. Wave 1 (03-01, 03-02) is parallel: the gate pack touches gates/** + gate.yml + a nano-memory test target plus store.rs/types.rs; the admission/policy plan touches nano-activation + nano-cli policy plumbing. No shared files. MS-05 LOCUS (decided, F3): the configured-agent registry is confirmed absent repo-wide and §6.8 forbids a registry table in the store. The split is fixed — (a) parse-error legs already hold in types.rs and are asserted, not rebuilt; (b) 03-01 adds store-open/session-start and write refusals driven by a configured-agent set plus active agent supplied BY THE CALLER via a named store.rs/types.rs signature change; (c) the host-side registry reader (`$NANO_HOME/agents/*.agent.toml`, §6.8 config-file precedent) lives in 03-02's policy module; (d) 03-03 repeats the session-start refusal through the real host seam, proving plumbing rather than supplying the first enforcement. §5 MemoryPolicy stays frozen — configured agents and active agent are separate caller parameters, never policy fields. A weakening of any existing check is never in scope.
2. Wave 2 (03-03) starts only after 03-02's summary lands (policy source + admission semantics are seam inputs). The journal-topology decision record precedes any wiring; the choice propagates to 03-04's equivalence obligations.
3. Wave 3 (03-04, 03-05) is parallel after 03-03: migration/equivalence vs measurement touch disjoint surfaces (nano-cli migration + nano-memory tests vs scripts/soak + docs/evidence). Both consume 03-03's seam; 03-04 also consumes the 03-03 decision record.
4. Wave 4 (03-06) requires 03-01 (cards), 03-04 (equivalence), and 03-05 (report) summaries. It runs gate-all, the 7-leg CI, the exact-artifact cross-repo run through the Phase 2 Desktop verifier pipeline shape (Desktop consumption notes only — no Desktop code), the default-off preservation manifest, and the disclosed-governance protected merge with a non-self-referential committed manifest and external final receipt.
5. Wave 5 (03-07): a separately spawned ferrox-verifier alone authors 03-VERIFICATION.md from a fresh merged checkout and the external receipt; only an all-VERIFIED PASS flips REQUIREMENTS/ROADMAP/STATE and marks Phase 4 ready-to-plan.

## File Ownership Conflict Audit

| Plan | Lane | Exclusive write surface | Overlap result |
|---:|---|---|---|
| 01 | gates | `gates/mem-sec/**`, `gates/fixtures/mem-sec/**`, `gates/registry.json`, `gates/tests/gates-mem-sec.test.cjs`, `crates/nano-memory/tests/{mem_sec_cards,corrective_regressions,durability,retrieval_recall,write_mediation}.rs`, `crates/nano-memory/src/{lib,store,types}.rs` (MS-05 required public signature, call-site updates, and retrieval checkpoint assertions only), `.github/workflows/gate.yml` | gate.yml is exclusive to 01 this phase; registry.json single-writer. nano-memory lib/store/types and required call-site updates are exclusive to 01 in Wave 1 (03-02 consumes the signature but does not edit nano-memory). |
| 02 | admission | `crates/nano-cli/src/memory_policy.rs` (incl. the §6.8 host registry reader), `crates/nano-cli/src/activation.rs`, `crates/nano-cli/src/acp_mode.rs` (policy plumbing only), `crates/nano-activation/src/admission.rs`, admission/cli test targets | acp_mode.rs shared with 03-03 by wave separation (02 lands first, disjoint regions: policy resolution vs seam wiring). |
| 03 | seam | `crates/nano-cli/src/memory_seam.rs`, acp_mode/host_mode/exec_run seam points, activation.rs binding, seam + quarantine tests (incl. the MEM-SEC-5 session-start leg at seam time), 03-03-DECISION.md | Sequential after 02; activation_quarantine.rs shared with 04 by wave separation (03 extends first, 04 extends after). |
| 04 | migration | `crates/nano-cli/src/memory_migrate.rs`, main.rs wiring, migration test, corrective_regressions.rs, activation_quarantine.rs (post-migration rows) | Disjoint from 05; quarantine file serialized by wave order within Wave 3 — 04 owns it in Wave 3. depends_on includes 03-01 (its verify runs the mem_sec_cards target). |
| 05 | measurement | `scripts/soak/continuity*.{mjs,json}`, `scripts/soak/test-continuity.mjs`, `docs/evidence/phase3/continuity-modes-report.md` | Disjoint from 04. |
| 06 | closure | `docs/evidence/phase3/phase3-*.json`, `default-off-preservation.json`, external receipt | No source overlap; consumes summaries. |
| 07 | closure | 03-VERIFICATION.md + ledgers | Verifier-owned artifact; ledgers flip only on PASS. |

## Execution Time Discipline

1. CI is a confirmation, never a debugger. Local gates green before any push. A CI-only failure's next action is a minimal LOCAL reproduction of the failing predicate; a second CI push without a local repro is a rule violation.
2. Round-trip test before integration test: every new wire format / ledger record / state machine gets a serialize→parse (or apply→replay) round-trip unit test in the SAME commit as the type.
3. Cheapest distinguishing experiment: hypothesis sentence before attempt 2, and attempt 2 must be the smallest experiment discriminating the live hypotheses — never a full-pipeline rerun. Original strike three triggers mandatory Kimi K3 + Claude Fable cross-research rather than immediate stop. The reconciled fix receives a fresh three-attempt remediation counter; remediation attempt 2 requires a new hypothesis and attempt 3 requires an isolated proof. Stop/handoff only after remediation strike three fails. Full suites run once per plan at the end, focused tests per task.
4. Per-plan wall-time tripwire: each plan carries a `wall_time_tripwire` frontmatter line — 03-01: "3 working days", 03-02: "1 working day", 03-03: "3 working days", 03-04: "2 working days", 03-05: "2 working days", 03-06: "2 working days", 03-07: "1 working day" — plus a 45-minute diagnosis time-box per failure class, then handoff. Hitting the tripwire = stop + .continue-here.md = SUCCESS, not failure.
5. Machine-checkable handoffs: every .continue-here.md entry carries the exact next command AND its expected output, so a resumer verifies state with one command instead of re-deriving context.

## Three-Strikes and Handoff

Attempts are counted per identical failing card/check, mutant class, CI leg, admission row, migration row, equivalence row, harness mode/seed, or verifier query. Before attempt 2 record one root-cause sentence. Before attempt 3 produce an isolating reproduction varying one factor. Original strike 3 ends local variation and triggers mandatory cross-research with Kimi K3 and Claude Fable. Reconcile their findings, record the selected fix, and start a fresh remediation counter capped at three implementation attempts. Before remediation attempt 2 record the researched fix's failed assumption and new hypothesis; before remediation attempt 3 prove the new variable in isolation. Only remediation strike 3 stops and writes `.continue-here.md` with exact worktree path/branch/base/head, dirty diff, commands, fixture/mutant/CI IDs, research findings, hypotheses/proof, exact next command, and prohibited retries. Fixture-label tuning to make a bar pass IS the failure class: it triggers contract-amendment discussion, never a fixture edit.

## Completion Bar

Plan 03-01 additionally requires `just gate-all` green on its final local PR head
before the first push. The summary is finalized in commit 2 before that exact-head
run; the external PR receipt records the SHA-bound result without changing the tested
tree. Focused failures are isolated before any full-gate rerun.

All of: six mem-sec checks green through the WP-3 verify entry with full mutant pools caught; fixtures/labels committed separately from implementation under §6.6 owner review; the four gate.yml inventory points plus 7-leg mem-sec coverage green; MemoryRecall admitted with pinned fallback and unchanged session_resume/fresh semantics; one seam across all four entrypoints with mediation-only model writes and principal_id≡agent_id byte-for-byte; legacy store resolved by documented decision with zero authority-regain vectors; old-DB/rebuild query-equivalence including agent_id under the recorded journal topology; seeded NDJSON continuity report with frozen budgets and an explicit recommendation; default-off preservation proven by external oracles; exact-artifact cross-repo run and disclosed-governance protected merge with non-self-referential manifest and external final receipt; independent ferrox-verifier PASS from a fresh merged checkout before any ledger moves. Phase 4 remains untouched.
