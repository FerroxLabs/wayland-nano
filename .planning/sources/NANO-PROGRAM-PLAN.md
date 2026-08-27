# NANO-PROGRAM-PLAN.md — the memory + persistent-agents master build plan

**Date:** 2026-08-26 · **Author:** Lane G (synthesis) · **Status:** governing build
plan, pending owner sign-off on MEMORY-CONTRACT v1.2 and the two open owner calls (§4).

**Authority:** this plan sequences ONE program — the WP-6 OWNER OVERRIDE (2026-08-23)
killed the deferral model: every formerly-deferred item is a committed work package
here, each shipping with the same verified/gate-carded bar as the core. Sequencing
exists only because later packages genuinely build on earlier ones. The SCOPE BOUNDARY
(2026-08-25) holds throughout: **Nano is the engine; bot-product UX is Desktop's.**
Every "absorb" item below passes the design test — *is this an engine primitive Desktop
can drive over ACP/config?* — or it is not in this plan.

**Inputs (every work package traces to one of these — no invented scope):**
WP-6 gate + OWNER OVERRIDE + SCOPE BOUNDARY (`WP6-DECISION-GATE.md`) · MEMORY-CONTRACT
v1.2 (`specs/MEMORY-CONTRACT.md`) · PROFILES-CONTRACT v1.0 · NANO-MODULE-CONTRACT v1.0 ·
Lane F (`MEMO-multi-bot-memory.md`) · Lane L (`bots/MEMO-graph-engineering.md`) ·
Lane M (`bots/DESIGN-persistent-subagents.md`) · Lane N
(`bots/MEMO-grokbot-hermes-bots.md`) · Lane O (`bots/MEMO-openbot-openmausbot.md`) ·
AUDIT-1/2/3 (`bots/`) · owner decisions 2026-08-26 (execution-backend flavors;
learn-from-doing; blast_radius labeling).

---

## 1. Governing owner decisions (fixed inputs — not re-opened by any WP)

1. **No deferrals.** Evidence gates control ACTIVATION, not EXISTENCE: build thin,
   measure on our own fixtures, activate-if-wins / ship-dark-with-documented-negative
   -result. Skipping the build is never the outcome. [OWNER OVERRIDE item 1; AUDIT-3]
2. **Nano = engine.** Bot-product UX (roster UI, creation flows, group chats,
   marketplace surfaces) is Desktop. Nano ships the primitives. [SCOPE BOUNDARY]
3. **Execution backend: BOTH container flavors from day one** — `browser` (headless
   Chromium + CDP screencast, cheap) and `desktop` (full XFCE + VNC, power) — selected
   per agent/profile, behind ONE seam (`host | browser-container |
   desktop-container`; a remote VM is a later hosted lane of the same seam, not an
   alternative architecture). [Owner decision 2026-08-26; Lane O Imp. 1, 6]
4. **Learn-from-doing = procedure extraction** from journals into the T2 `procedures`
   table — model-proposes/host-commits, trust-tiered, visible receipt
   (MEMORY-CONTRACT §6.7). [Owner decision 2026-08-26]
5. **`blast_radius` ships as suggested/labeled** with measured confidence, never
   asserted fact. The precision eval on wayland-nano itself measures the FP rate; the
   measured rate becomes the label's stated confidence. [Owner decision 2026-08-26;
   OWNER OVERRIDE item 3; Lane L §2.3; AUDIT-3]

## 2. Build order at a glance

| # | WP | Name | Hard dependencies | Can run parallel with |
|---|---|---|---|---|
| 0 | WP-0 | Contract freeze (v1.2 signed) | — | nothing — blocks everything |
| 1 | P-MEM-1 | Core store: schema, resolver, journal, BM25+KNN retrieval | WP-0 | P-GRAPH-1 (separate lane) |
| 2 | P-MEM-SEC | `mem-sec` gate pack (6 cards) | P-MEM-1 (fixtures may be authored in parallel) | P-PROF, P-MOD-GAP |
| 3 | P-PROF | Profiles: merge math + `Op::ProfileSet` | P-MEM-1 | P-MEM-SEC, P-MOD-GAP |
| 4 | P-MOD-GAP | Module manifest gap closure | WP-0 | P-MEM-1, P-PROF |
| 5 | P-BOT-5a | Named agents: registry, ceilings, persona, roster primitives | P-PROF, P-MOD-GAP | P-MEM-SEC, P-CONS |
| 6 | P-BOT-5b | Fork-chain resume + memory accumulation | P-BOT-5a, P-MEM-1 | P-EXE-1, P-CONS |
| 7 | P-BOT-5c | Proactivity: routines, escalation, typed activation failures | P-BOT-5b | P-EXE-1, P-EXE-2 |
| 8 | P-EXE-1 | Execution-backend seam + `browser` flavor | P-BOT-5a | P-BOT-5b, P-BOT-5c |
| 9 | P-EXE-2 | `desktop` flavor (XFCE + VNC) | P-EXE-1 | P-BOT-5c, P-CONS |
| 10 | P-CONS | Consolidation: memory-aware compaction extraction | P-MEM-1 | P-BOT line, P-EXE line |
| 11 | P-PROC | Procedure extraction (learn-from-doing) | P-MEM-1 | P-BOT line, P-CONS |
| 12 | P-GRAPH-1 | Code-structure index — **the repomap rewrite** | WP-0 | everything (own lane) |
| 13 | P-GRAPH-2 | Suggested `blast_radius` behind precision eval | P-GRAPH-1 | P-BOT/P-EXE lines |
| 14 | P-MEM-KG | KG-BFS third RRF leg (thin, fixture-gated) | P-MEM-1, extended fixture | P-GRAPH-2, P-EXE-2 |
| 15 | P-XPROJ | Cross-project opt-in reads | P-MEM-1, P-MEM-SEC | nothing — built LAST |

The ordering of graph packages follows Lane L Imp. 2 exactly: code-structure index →
suggested blast_radius → KG-BFS memory leg. Cross-project reads are last so the
partition machinery is gate-carded in anger before any sanctioned path around it
exists (MEMORY-CONTRACT §6.3).

## 3. Work packages

### WP-0 — Contract freeze

- **Scope:** owner review + signature of MEMORY-CONTRACT v1.2 (the agent_id schema,
  write-authority rule, activation-not-existence clause, persona section — all folded
  in per AUDIT-2 C7: signing v1.1 re-creates finding A2 one level up). Housekeeping
  tail from WP-6 Phase 0 if any remains open.
- **Contract refs:** MEMORY-CONTRACT §12/§13.
- **Acceptance evidence:** signed contract; `gates/**` CODEOWNERS entry verified on the
  path (§6.6 — including the new `agents/**` extension).
- **Tripwire:** any lane building against v1.1 text after v1.2 is published = stop and
  re-baseline (the A2 failure shape).

### P-MEM-1 — Core memory store

- **Scope:** `nano-memory` crate — SQLite + FTS5 + sqlite-vec; bi-temporal
  facts/decisions/episodes/procedures with `source_trust`, `project`, **`agent_id`
  from day one**; deterministic tier-aware resolver with conflict domain
  `(project, agent_id, subject, predicate)`; `Op::MemoryWrite*` journal family
  (integrator lane) with `agent_id` payloads; retrieval passes 1–2 + RRF + tier
  down-weighting + `agent_scope`; §6.7 write-authority mediation point
  (model-proposes/host-commits, visible receipt); retention caps keyed
  `(project, agent_id)`; `kg_nodes`/`kg_edges` tables shipped unpopulated.
- **Contract refs:** MEMORY-CONTRACT §1–§6, §11.
- **Acceptance evidence:** §11 bars 1 (recall@10 ≥ 0.90 on
  `memory-retrieval-recall-v1`, zero cross-project AND zero cross-agent rows), 2
  (kill-mid-write durability, query-equivalent incl. `agent_id`), 4 (write-mediation
  gate test); rusqlite+sqlite-vec green on all 7 CI legs (Windows ARM64 is the
  predicted failure point — WP-6 spike precondition).
- **Dependency:** WP-0.
- **Parallelization:** memory lane works alone in `nano-memory`; integrator adds the Op
  family in nano-session concurrently against the frozen types; P-GRAPH-1 proceeds in
  the graph lane without touching this crate.
- **Tripwire (inherited from WP-6, Judge 4):** not clearly landing within ~5 working
  days of WP-0 → stop, ship what exists, re-run WP-6 with profiles leading. Scope
  smell: anyone re-adding KG-BFS, hosted embeddings, or compaction extraction to this
  WP "while we're in here" — those are their own packages (§1 decision 1).

### P-MEM-SEC — `mem-sec` gate-card pack

- **Scope:** the six §6.5 cards (poisoned supersession, same-tier control,
  cross-project leak, extraction laundering extended to bot attribution,
  removed-scope escape, **MEM-SEC-6 cross-bot leak**) on the WP-3 verify CLI / WP-4
  gate-card machinery; fixture rows committed with the pack.
- **Contract refs:** MEMORY-CONTRACT §6.5, §6.6, §11.3.
- **Acceptance evidence:** pack green on all 7 CI legs; §6.6 owner review of the
  `gates/**` change (the pack growing to six cards is itself a reviewed gates change).
- **Dependency:** P-MEM-1 for execution; fixture/card authoring parallel with P-MEM-1.
- **Tripwire:** a card weakened to pass ("seed fewer rows", "assert at output only")
  is a §6.6 violation — the checkpoints are pass-1, pass-2, AND assembled output.

### P-PROF — Profiles

- **Scope:** PROFILES-CONTRACT v1.0 as written — closed TOML struct, merge math
  (lattice min, set intersect/union, extends narrow-only), `Op::ProfileSet`,
  resume-narrows, the three shipped profiles. **Plus the §6.9 amendment:** the
  `system_prompt_file` "narrow-only ref" clause is dropped and restated as
  overlay-only over the kernel-owned core (AUDIT-2 C3 — amend PROFILES-CONTRACT to
  v1.1 at build time, same version-stamp discipline).
- **Contract refs:** PROFILES-CONTRACT §1–§7; MEMORY-CONTRACT §6.9.
- **Acceptance evidence:** the contract's own battery — merge-math tests, fail-closed
  resolution, journal roundtrip + resume-narrows (revoked tool stays revoked across
  kill-resume), three shipped profiles behave, `just gate-all` green.
- **Dependency:** P-MEM-1 (WP-6 fast-follow ordering; Lane M §4 sequencing).
- **Tripwire:** ≤3 working days. Scope smell: `memory`/`egress`/`mcp_servers` fields
  (contract v2) creeping into v1.

### P-MOD-GAP — Module manifest gap closure

- **Scope:** close the gap BEFORE any agent file may name `[modules]` (Lane J item 3,
  ADOPTED by AUDIT-2): `contract_version` on the plugin manifest (AUDIT-1 J-15:
  zero hits today); digest-pinned sources (content-digest source pin per AUDIT-1
  D-2's corrected reading: `RegistrySource{LocalDir,Github}` exists at crate level,
  the manifest schema needs the digest variants + install-time verification);
  install-receipt `Op`; provenance wiring. **Registry-kind governance** (AUDIT-2 M4):
  until a curated first-party registry with a governance story exists, the
  `registry` source kind returns a typed unimplemented refusal — honest over a
  curl-pipe by another name.
- **Contract refs:** NANO-MODULE-CONTRACT §1–§3 (amend to v1.1 with the new fields);
  AUDIT-2 M4.
- **Acceptance evidence:** tampered-digest module fails closed at install; install
  receipt journaled with digest; `registry`-kind manifest returns the typed refusal;
  `just gate-all` green.
- **Dependency:** WP-0 only — highest parallel value in the program.
- **Tripwire:** ≤2 working days. Smell: designing the registry governance itself
  (that is a P-ASK, not this WP).

### P-BOT-5a — Named persistent agents, landing 1 (composition)

- **Scope (Lane M §2, §4):** `agents/*.agent.toml` registry (one `agent_id` string,
  PROFILES grammar, `main` reserved; filename-stem match; fail-closed validation);
  `task_spawn` gains `agent_id`; resolved composition (agent_id + config hash +
  resolved ceiling + module digests + persona hash) journaled in the child's
  `SessionBegin`; ceiling math `min(spawning context mode, extends-chain, ceiling)`;
  persona overlay over the kernel-owned core (fail-closed load); `ChildUsageRollup`
  extended with `agent_id`; roster primitives the parent model can address
  (host-side selection only — the model never creates agents or self-assigns
  identity); `nano agent create <id>` three-field scaffold (Lane N delta 5). Dogfood
  trio: `assistant`, `reviewer`, `researcher` (Lane J item 7 = Lane K's role trio —
  convergent). NO P-MEM dependency — children stay memory-less in this landing.
- **Contract refs:** MEMORY-CONTRACT §6.8/§6.9; Lane M §2.1–2.5, §2.7; PROFILES
  narrow-only math.
- **Acceptance evidence:** spawn-by-name with journaled composition hash; a
  project-declared agent file refuses without workspace trust; ceiling math tests
  (read_only parent ⇒ read_only activation regardless of file ceiling); persona
  overlay never displaces the core (resolved-prompt inspection); rollup carries
  `agent_id`; `agents/**` under the §6.6-style review rule.
- **Dependency:** P-PROF (extends-chain), P-MOD-GAP ([modules] refs).
- **Tripwire:** ≤5 working days. Smell: resume or memory tools sneaking in — that is
  5b; a second per-bot config format (Lane I item 7 was AMENDED to bot.toml-only).

### P-BOT-5b — Persistence + accumulation, landing 2

- **Scope (Lane M §2.3, §2.6, §2.8):** fork-chain resume — the host forks the agent's
  most recent completed run journal (`fork.rs`, digest-asserted, fail-closed);
  postures re-derived never restored (ModeSet precedent); identity-checked resume
  (persona/config hash mismatch = typed refusal + explicit `rekey` acknowledgment);
  compaction-on-import past the threshold; one live activation per `agent_id`
  (`AgentBusy`); per-agent memory — scoped recall tools (default `Own`) and
  `memory_propose` through the §6.7 mediation point; per-agent ledger folding
  rollups; visible memory-write receipt as a REQUIRED UX element (Lane N delta 7 —
  triple-confirmed category standard); `nano agent gc` with export-before-prune.
- **Contract refs:** MEMORY-CONTRACT §3 (agent_scope), §4 (journal payloads,
  concurrency model), §6.7; Lane M §3 failure modes.
- **Acceptance evidence:** resume reconstructs continuity bit-for-bit (fork digests);
  a revoked module/ceiling stays revoked across resume; MEM-SEC-6 green in the pack;
  a compromised-agent scenario demonstrates per-bot surgical rollback
  (`DELETE … WHERE agent_id=… AND valid_from > …`); ledger output reconstructible
  from the parent journal alone.
- **Dependency:** P-BOT-5a, P-MEM-1. **Owner call Q2 (§4) shapes the resume default —
  the WP builds both paths; the call picks the default.**
- **Tripwire:** ≤5 working days. Smell (Lane M §3.4, Anthropic's 15×): one-shot
  fan-out work migrating onto persistent agents "because they exist" — persistent
  agents are for recurring, accumulating work; ephemeral tasks stay.

### P-BOT-5c — Proactivity: routines + escalation

- **Scope (Lane N deltas 1, 2, 4, 6 — the absorb list; all engine primitives Desktop
  drives over ACP/config):** cron jobs naming an `agent_id` — firing is a normal 5b
  activation, namespaced `[agent:<id>] <routine>`, mode-capped
  `min(session_mode, default)`; **routine activations are memory-primary** (fresh
  context + scoped recall, journals chained for audit — Lane N delta 3 resolves Q2
  for this case); per-agent routine cap + bounded run-record retention (starting
  points 50/20 — both category products converged); routine receipts with prompt
  snapshot (an edited definition cannot rewrite history — Lane O Imp. 12), cost,
  denials; `Op::AttentionRequested { agent_id, reason, severity }`, journaled,
  rate-limited, surfaced to TUI/Desktop as needs-you; typed activation-failure
  reasons + retry-once-with-compaction; `nano agent pause <id>` global kill switch.
- **Contract refs:** Lane N Part 6; MEMORY-CONTRACT §4 (concurrency model — routines
  respect `AgentBusy`).
- **Acceptance evidence:** a routine fires, activates the agent, lands receipts
  (prompt snapshot + cost + denials) in the agent's ledger; escalation op journaled
  and rate-limited under a loop-spin test; typed failure reasons distinguish
  retry-later from reconfigure; spin caps enforced in code, not convention.
- **Dependency:** P-BOT-5b.
- **Tripwire:** ≤4 working days. Smell: event triggers / webhook receiver growing a
  broader API (Lane O Imp. 12's isolation pattern is the ceiling: dedicated loopback
  port, `/health` + secret hook paths only); teach-by-demonstration appearing in ANY
  form (Lane N delta 8 — hard reject).

### P-EXE-1 — Execution-backend seam + `browser` flavor

- **Scope (owner decision 3; Lane O Imp. 1–9):** the backend enum
  (`host | browser-container | desktop-container`) on the agent composition,
  journaled at spawn in the resolved-composition payload (`computer_backend`);
  **supervisor vocabulary constraint** — whatever holds the container-runtime socket
  exposes ensure/stop/reset/list addressed by validated `agent_id`, names derived
  from the id, never caller-supplied ("the vocabulary is the boundary");
  **digest-pinned image** (base by digest, added layers SHA-256-verified,
  version-stamped labels, image ref inside the journaled composition hash);
  **hardening re-verified at inspect time** before every use — resource limits,
  cap-drop-ALL + minimal add-back, private IPC/cgroup ns, exactly one workspace bind
  mount, loopback-only viewer — mismatch = typed `unsafe` refusal (posture is
  re-derived, never remembered); **`browser` flavor**: headless Chromium + workspace
  + shell, observation by CDP screencast (JPEG frames, ack backpressure — strictly
  less machinery than a VNC stack); **frame-digest receipts** — a frame captured at
  each action completion, content-addressed, digest in the CUA op payload, 4MB
  refuse-don't-truncate; **TTL leases** per computer (a dead provider cannot pin it);
  **capability-intersection** — mounted tool set = agent grants ∩ backend
  capabilities, journaled (never tell an agent it has a computer its backend can't
  mount); refuse-not-queue under human control, journaled as control ops.
- **Contract refs:** Lane O Imp. 1–9; owner decision 3; MEMORY-CONTRACT §7 (the seam
  is engine machinery, not Desktop surface).
- **Acceptance evidence:** supervisor refuses any verb outside the four; hardening
  re-inspection refuses a tampered container (typed error); a journal answers "what
  computer produced this work" (image digest in composition hash); frame receipts
  keyed (agent_id, op_id, target) replay a past action's frame, not the live screen;
  vision-token metering (captured vs sent-to-model) present as a budget line.
- **Dependency:** P-BOT-5a (the composition resolver it extends). Parallel with
  5b/5c.
- **Tripwire:** ≤5 working days for seam + browser flavor. Smell: building a
  policy-expression language (OpenBot's CEL gateway is the v2 reference — Lane O
  Imp. 8 names it as the computer-action policy layer, a LATER package; v1 reuses the
  existing lattice + approval gate); remote/cloud backends (later hosted lane of the
  same seam).

### P-EXE-2 — `desktop` flavor (XFCE + VNC)

- **Scope (owner decision 3; Lane O Part 2 — OpenMausBot's Local VM is the reference
  implementation):** full XFCE desktop container + CUA driver inside + noVNC out;
  recreate-don't-resume (a stopped desktop image cannot safely resume — typed 409,
  restart policy `no`); AX-element paths preferred over pixel coordinates;
  who-is-driving control seam pausing the agent's hands mid-turn. Same seam, same
  supervisor vocabulary, same hardening contract, same receipts as P-EXE-1 — this WP
  is the second backend, not a second architecture.
- **Contract refs:** Lane O Imp. 1, 3, 5–7; owner decision 3.
- **Acceptance evidence:** per-agent selection (`browser` vs `desktop`) from the
  agent/profile composition, journaled; recreate-not-resume refusal is typed;
  hardening checklist shared with P-EXE-1 (one contract, two flavors); the
  `dockerSecurityIsHardened`-style inspection passes on Docker AND Podman.
- **Dependency:** P-EXE-1.
- **Tripwire:** ≤4 working days — the reference implementation exists; this is
  catch-up with receipts, not research (Lane O Imp. 13: build thin, steal
  shamelessly, spend saved effort on P-MEM). Smell: host-control backend creeping in
  (opt-in host control is OpenMausBot's fourth backend; it is NOT in owner decision
  3's day-one set).

### P-CONS — Consolidation: memory-aware compaction

- **Scope (MEMORY-CONTRACT §8; OWNER OVERRIDE item 5):** the consolidator extracts
  facts/decisions from compacted spans into T2 — under the §6.7 write rule
  (model-proposes/host-commits, ModelInference cap, visible receipt);
  `memory.compaction_extraction` flag becomes functional (typed error today);
  deterministic-first sweep (Lane I item 5); **the F-45 conversation-retention
  policy question is a design input here** (~8 KB/turn retained growth is
  closed-as-measured; what consolidation keeps vs lets age out is this package's
  design brief); the extraction-model cost question (session model vs cheap local —
  10×) is decided in this WP's design phase.
- **Contract refs:** MEMORY-CONTRACT §8, §6.7, §10.1.
- **Acceptance evidence:** extraction writes land only via §6.7 mediation
  (ModelInference-capped, journaled, receipted); a compaction round-trip shows
  extracted rows retrievable post-compaction; retention policy stated and enforced
  by the §4 caps; cost measurement reported against the 10× question.
- **Dependency:** P-MEM-1. Parallel with the P-BOT/P-EXE lines.
- **Tripwire:** ≤4 working days. Smell: extraction writing at ToolOutput tier or
  higher "because the transcript is real" — LLM-mediated transformation caps at
  ModelInference, no exceptions (§6.1); LLM-in-the-loop ingest on the WRITE path
  (§9 anti-scope — extraction here is off the hot path).

### P-PROC — Procedure extraction (learn-from-doing)

- **Scope (owner decision 4; MEMORY-CONTRACT §6.7):** mine session journals for
  repeated successful procedure shapes; extract candidates into the T2 `procedures`
  table — model-proposes/host-commits, trust-tiered (ModelInference cap), visible
  receipt per committed procedure; procedures scoped `(project, agent_id)` like every
  other content row; retrieval surfaces procedures through the existing passes.
- **Contract refs:** MEMORY-CONTRACT §1 (procedures row), §6.7; owner decision 4.
- **Acceptance evidence:** a fixture journal with a repeated task shape yields a
  proposed procedure; host commit lands it ModelInference-tiered with receipt; the
  procedure is retrievable by a later session of the same `(project, agent_id)` and
  invisible to others; no procedure lands without the receipt.
- **Dependency:** P-MEM-1. Parallel with P-BOT/P-CONS. Benefits from P-BOT-5b (more
  journals to mine) but does not require it.
- **Tripwire:** ≤3 working days. Smell: auto-EXECUTION of learned procedures (this
  WP produces retrievable knowledge, not automation — routines that act are
  P-BOT-5c's, and captured-procedure automation remains governed by Lane N delta 8's
  versioned-diffable-artifact rule).

### P-GRAPH-1 — Code-structure index (THE REPOMAP REWRITE — owner lane named here)

- **Owner lane: the GRAPH LANE** (MEMORY-CONTRACT §7, as amended v1.2 — this closes
  the ownership gap AUDIT-3 flagged: WP-6 established nano-repomap is regex-based
  and the edge extension "is a rewrite, not an extension," and no lane owned that
  rewrite; the graph lane owns it, may depend on `nano-memory` types crate-level,
  and nano-repomap ships unchanged until this package lands).
- **Scope (Lane L Imp. 2a, 4):** tree-sitter def/ref index feeding repomap ranking —
  aider pattern (defs/refs, Personalized-PageRank-style rank-into-token-budget
  serialization) PLUS the xAI donor's engineering (string interner, debounced
  incremental reindex, versioned binary cache, query-version-hash invalidation,
  git-tracked-only builds, CI RSS budgets) — explicitly NOT the donor's
  string-equality cross-file "resolution," which is name-matching, not name
  resolution (Lane L §2.1 — the corrected donor story; AUDIT-3 verified the
  correction exhaustively).
- **Contract refs:** Lane L §2, Imp. 2/4; MEMORY-CONTRACT §7. Evidence basis: the
  SuperCoder ablation (acc@5 44.3% → 84.5%, p<0.0001) — the strongest measured win in
  the field, and it is code-structure, not memory.
- **Acceptance evidence:** localization benchmark on wayland-nano itself (the
  repo-root scope question, MEMORY-CONTRACT §10.4, decided here); rank-into-budget
  serialization respects the token budget; incremental reindex correct under edit
  churn; CI RSS budgets enforced.
- **Dependency:** WP-0 only — own lane, parallel with the whole memory line.
- **Tripwire:** ≤6 working days. Smell: call edges or `blast_radius` appearing in
  this WP (that is P-GRAPH-2, behind its eval); LSP/SCIP machinery (§9 anti-scope —
  tree-sitter only).

### P-GRAPH-2 — Suggested `blast_radius` behind the precision eval

- **Scope (owner decision 5; OWNER OVERRIDE item 3; Lane L §2.3, Imp. 2b; AUDIT-3):**
  Rust-only call-edge extraction on the P-GRAPH-1 index, exposed as a **suggested,
  labeled** blast radius — presented as a heuristic with a stated confidence, NEVER
  as asserted fact. The precision eval on wayland-nano itself (the override's gate)
  measures the FP rate; **the measured rate becomes the label's stated confidence**
  (AUDIT-3's wiring of the two halves into one mechanism). Dynamic languages stay
  out — the ~83%-FP name-matching mode is a correctly-cited trap.
- **Acceptance evidence:** the precision-eval report on wayland-nano (committed, with
  the fixture); every `blast_radius` output carries the measured-confidence label;
  no surface presents edges as fact; activation-if-the-eval-supports-it per §1
  decision 1 — a bad eval ships the tool dark with the documented negative result.
- **Dependency:** P-GRAPH-1.
- **Tripwire:** ≤3 working days. Smell: "just one dynamic language" (the evidence
  says no); upgrading the label to an assertion (owner decision 5 is fixed).

### P-MEM-KG — KG-BFS third RRF leg (thin, fixture-gated)

- **Scope (MEMORY-CONTRACT §3 pass 3 — the verbatim activation clause governs):**
  build thin — entity nodes extracted deterministically from fact subjects/objects,
  typed edges from predicates, BFS depth ≤2, hard token budget (~5–7k), RRF k=60;
  populates the already-shipped `kg_nodes`/`kg_edges` (with `agent_id`); wire behind
  retrieval config; measure against the EXTENDED `memory-retrieval-recall-v1`
  fixture in CI. **Activate-if-wins; ship-dark-documented if it loses.** Before
  activation, the relation-level poisoning card joins `mem-sec` (Lane L Imp. 6 —
  GragPoison: passage-level defenses do not transfer to graph retrieval).
- **Acceptance evidence:** the CI measurement against the extended fixture (either
  way — win or documented negative result); depth/token-budget invariants tested as
  contract-level; the relation-poisoning card green before activation; traversal-
  shaped provenance visible in pass-9 output when active.
- **Dependency:** P-MEM-1 + the extended fixture (§11.1); Lane L's ordering puts it
  after the code-graph packages.
- **Tripwire:** ≤3 working days — the tables already ship; this is retrieval-side
  code + a gate card (AUDIT-3: there is no "save the work by not building" option
  worth taking). Smell: extending past depth 2 (the evidence is exponential volume,
  degrading accuracy — Lane L Imp. 5); community detection or LLM entity resolution
  (§9 anti-scope).

### P-XPROJ — Cross-project opt-in reads (built LAST)

- **Scope (MEMORY-CONTRACT §6.3, §9):** `ReadScope::Global` returns ONLY as an
  explicit, logged, per-query opt-in — journaled on every use, profile-tightenable,
  never a silent widening. Built last so the partition predicate and its gate cards
  (MEM-SEC-3/6) have been green in CI across the entire program before any
  sanctioned path around them exists.
- **Acceptance evidence:** opt-in query journaled with scope, caller, and reason;
  default behavior bit-identical to pre-P-XPROJ (the mem-sec pack re-run green);
  profiles can disable the opt-in entirely (tighten-only rule intact).
- **Dependency:** P-MEM-1, P-MEM-SEC. Last in the program.
- **Tripwire:** ≤2 working days. Smell: opt-in becoming sticky/config-level (the
  whole point is per-query); cross-AGENT reads riding along (that is the A2A
  package's schema door — AUDIT-2 C6 — not this one).

## 4. The two remaining owner calls — DECIDED 2026-08-25

### Q2 — Continuity substrate on resume — **DECIDED: Option B, memory-primary, for everything**

Owner chose memory-primary for BOTH interactive and routine activations: fresh context
each activation; continuity loads via scoped memory recall; journals chain for audit
only. The routine case was already resolved memory-primary (Lane N delta 3); this
extends it to the interactive default. Consequences: P-BOT-5b builds memory-primary as
THE resume path (transcript-replay becomes the audit/fallback path, not a second
verified default); §11 bar 1 (recall fixture) is load-bearing for the experience — the
token floor drops and memory quality becomes the product. The chain still needs gc
discipline for the audit trail.

### Q7 — Agent-id recycling — **DECIDED: Option A, never-recycle**

Owner chose never-recycle: re-creating a retired id is a typed error, forever. One
identity per id, ever — forensically clean attribution. Tombstone store = `retired`
marker file in `agents/` (config-side, no DB table). Per AUDIT-3, this also settles
`fresh: true`: reachable only for never-retired ids.

<details>
<summary>Original options considered (preserved for audit)</summary>

Q2 options: A — transcript-replay (fork chain + import, high token floor); B —
memory-primary (fresh context, recall-driven continuity); C — split by activation
type (interactive=A, routine=B). Owner chose B-for-everything.

Q7 options: A — never-recycle (typed error forever, tombstone marker file); B —
tombstone-with-grace (reuse after grace window, epoch-aware attribution). Owner
chose A.

</details>

## 5. Program-level anti-scope (traced rejects — visible so nobody re-proposes them silently)

Bot-product UX — roster UIs, creation flows, group rooms, marketplaces (Desktop —
SCOPE BOUNDARY) · bot-to-bot messaging day-one (AUDIT-2 C6; schema door stays open)
· teach-by-demonstration capture (Lane N; Lane O Imp. 11 — doubly rejected)
· shared-machine/shared-credential bot roster (Lane N reject; per-agent isolation is
the differentiator — Lane N item 10, Lane O Imp. 13) · forever-chat semantics (Lane N
— Nano's continuity is the fork chain + P-MEM rows) · model-based auto-approval
(Lane N hard reject — the lattice is deterministic) · cross-machine relay/mesh
(Lane N non-goal) · OpenBot's Postgres + external memory service (Lane O Imp. 10) ·
asserted tree-sitter blast radius on dynamic languages (Lane L §2.3) · embedded
graph DBs (Lane L §5 — Kuzu archived; SQLite CTEs confirmed at our scale) · MCP
exposure of memory (MEMORY-CONTRACT §9).

## 6. What this plan deliberately leaves non-executable (stated, not hidden)

- **Persona overlay residual risk.** Overlay-only limits but does not eliminate
  prompt-level behavioral widening; no mechanical check can prove one prose file's
  "posture" is a subset of another's. The hard floor is capability narrowing
  (MEMORY-CONTRACT §6.9). This is a stated residual, not a solved problem.
- **The compaction-on-import threshold** (50% of context budget) is a hand-wavy
  default — safe-directional (bounds re-read cost), tunable after measurement
  (Lane M OQ-5; AUDIT-3 accepts it as lane-decidable).
- **The 10× extraction-cost question** has no executable answer until P-CONS's
  design phase measures it — the plan assigns the decision, it cannot pre-make it.
- **Cost attribution currency** derives at render time ("prices change, journals
  don't" — Lane M OQ-6); pricing tables are config, not journal, so per-agent cost
  ledgers are only as current as the price config.
- **Routine/run-record cap numbers** (50/20) are category-convergent starting
  points, not measured optimums — enforced in code from day one, tunable by the
  owner without a contract amendment.

*End of plan. Signature of MEMORY-CONTRACT v1.2 + the two owner calls above unlock
WP-0 → P-MEM-1.*

