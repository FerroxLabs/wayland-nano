# Phase 3 Context: Runtime-Integrated Scoped Continuity

**Authority:** ROADMAP Phase 3, REQ-MEM-01, REQ-MEM-02, REQ-CONT-01, signed MEMORY-CONTRACT v1.2 (§3, §5, §6.1–6.9, §11), WORKABLE-AGENT-AUTHORITY-AMENDMENT v1.0, Phase 1 verification, Phase 2 verification (02-VERIFICATION.md, PASS), Phase 3 research (03-RESEARCH.md).

## Goal

Secure and wire real scoped memory: CLI and both Desktop ACP activation stacks traverse one `nano-memory` seam under `(project, principal_id)` semantics, the Phase-2-quarantined filesystem-memory path is migrated/disabled/subordinated without regaining authority, and continuity modes (`fresh`, `session_resume`, `memory_recall`) are measured so Desktop selects defaults from evidence. Stop before Phase 4 scheduler retirement and triggers.

## Locked Decisions

- D3-01: The `mem-sec` gate-card pack implements MEMORY-CONTRACT §6.5 verbatim — six cards (poisoned supersession, same-tier control, cross-project leak, extraction laundering incl. bot attribution, removed-scope escape, cross-bot leak), each asserting at pass-1 FTS, pass-2 KNN, AND final assembled output where the card says so. The pack runs through the existing WP-3 verify CLI / WP-4 gate machinery on all 7 CI legs. Fixture rows are committed with the pack; any `gates/**` change (cards, fixtures, registry) is a §6.6 owner-review change, and fixture + labels land in a separate commit/PR from the retrieval implementation (anti-self-grading). Fixture labels are never tuned to make a bar pass — that is the failure class, not a fix.
- D3-02: Wire `principal_id` remains 1:1 with physical/schema/journal `agent_id` byte-for-byte under the existing grammar (Phase 1 amendment, Phase 2 D2-05). No schema rename. Old-DB and journal rebuild must produce query-equivalent rows and receipts including `agent_id` (MEMORY-CONTRACT §11 bar 2; MEM-SEC-4).
- D3-03: Exactly one memory seam. The four Phase-2 quarantine points are replaced by `nano-memory`-backed wiring: authenticated ACP context injection (`acp_mode.rs:3607-3618`), authenticated ACP tool surface (`acp_mode.rs:4493-4505`), protocol-host (`host_mode.rs:291-294`), and exec (currently no memory layer, `exec_run.rs:309-322`). The legacy `<nano_home>/memory/*.md` store is migrated, disabled, or made subordinate; `MemoryToolExecutor::quarantined` backstops remain until migration evidence lands; `activation_quarantine.rs` is extended, never weakened. Legacy `Op::MemoryWrite*`/`Cron*` journal vocabulary stays replay-readable and grants no authority.
- D3-04: `ContinuityStrategy::MemoryRecall` stops being a hard `ContinuityNotEnabled` refusal (`admission.rs:884-893`) and becomes a validated mode with pinned fallback semantics in the signed carrier; `session_resume` drift checks (`validate_continuity`, `admission.rs:878-913`) are unchanged and remain the fail-closed template; `fresh` keeps requiring `fallback:none`.
- D3-05: Write authority is unchanged from the contract: host-originated writes commit directly; model-originated writes are proposals only through the §6.7 single mediation point (`commit_proposal`) — force-capped `ModelInference`, deterministic-only screening, journaled with `agent_id`/`source_trust`/`project`, user-visible receipt. No LLM screener, no MCP exposure of memory, no second write path.
- D3-06: Journal topology is decided before wiring: `MemoryStore::open(nano_home, journal_path, policy)` currently takes its own journal; whether memory ops ride the per-session `JournalCoordinator` or a dedicated memory journal is resolved explicitly in the seam plan, with kill-mid-write durability and `rebuild_from_journals` query-equivalence proven either way (store.rs fault-injection seam + corrective_regressions rebuild-equivalence pattern).
- D3-07: Continuity measurement is evidence, not a gate card: a harness on the `scripts/soak` fake-model chassis measures `fresh` vs `session_resume` vs `memory_recall` for quality/latency/tokens with recorded budgets; the report recommends defaults; Desktop remains the default-setting authority. The `memory-retrieval-recall-v1` fixture stays the honest recall instrument — labels are never changed in response to retrieval output.
- D3-08: Anti-scope (stop/replan tripwires): no schema rename, no hosted memory/embeddings, no MCP expansion, no compaction/procedure extraction, no graph/KG work (`kg_nodes`/`kg_edges` stay unpopulated — pass-3 KG-BFS is gated behind a future seventh card and its own eval), no `ReadScope::Global`/cross-project reads, no cross-agent reads beyond the contract's `agent_scope`, no scheduler/registry/UI/provider/product-control-plane work, no duplicate bootstrap path.
- D3-09: CI ownership: `gate.yml` hardcodes the gate-card inventory in four places (assert-list, C# allowlist, capability probes, run loop) and currently runs gate cards Windows-only; adding `mem-sec` updates all four points and extends the pack to all 7 CI legs per §6.5. This is a CI-ownership change and rides the same protected-review path.
- D3-10: Governance and discipline carry over unchanged from Phases 1–2: separate authorized worktrees; Nano before Desktop; owner-directed agent-operated TradeCanyon review/merge with full disclosure; evidence bar (tests green + 7-leg CI + receipts); three-strikes root-cause rule; exact handoff on stop; no Phase 4 scope (scheduler retirement, triggers, dogfood).

## Acceptance

- REQ-MEM-01, REQ-MEM-02, REQ-CONT-01 are mapped exactly once to executable plans.
- MEM-SEC-1–6 pass at FTS, KNN, and final assembly with zero cross-project and zero cross-agent leakage, attribution preserved, on all 7 CI legs.
- CLI and both Desktop ACP stacks traverse the same `nano-memory` seam; legacy filesystem memory cannot regain authority; typed negatives prove the old routes stay closed.
- Old-DB and journal rebuild are query-equivalent including `agent_id`; receipts round-trip.
- Continuity-mode report with measured quality/latency/tokens and a recommended default exists; `session_resume` fails closed on composition/policy drift.
- Exact-artifact cross-repo run, both CI systems, protected merges, and independent phase verification — same closure shape as Phase 2 (verifier-authored VERIFICATION, ledger flip only on PASS).

## Discipline

- Separate Nano and Desktop worktrees/PRs; `gates/**` and `agents/**` changes get §6.6 owner review; agents propose but never approve gate cards certifying their own work.
- No `.secrets` reads. No tags or hidden merge/bypass.
- Three strikes per repeated failure; exact `.continue-here.md` handoff on stop.
- Scope smells named in the roadmap anti-scope are stop/replan, not "while we're here."
