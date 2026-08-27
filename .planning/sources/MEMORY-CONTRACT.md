# MEMORY-CONTRACT.md — the P-MEM interface freeze (2026-08-20; amended 2026-08-24; amended 2026-08-26)

**Authority:** this document is the ONLY lane-split authority for the memory program
(P-MEM, Stage 2). Where any lane's spec disagrees, this file wins. It exists because the
0.2 plan audit (finding A2) proved two lanes without a frozen interface build
incompatible pieces. **P-MEM does not start until this contract is owner-signed.** The
WP-6 gate (2026-08-23) returned PROCEED to a scoped milestone ("P-MEM-lite") gated on
Phase-0 preconditions; v1.1 folded the gate's four security items and the panel's scope
cuts into this contract. The WP-6 **OWNER OVERRIDE** (2026-08-23) then rejected the
deferral model: every cut item is a committed work package inside one memory program
(sequenced in `../NANO-PROGRAM-PLAN.md`), and multi-bot identity is a **day-one schema
requirement**. v1.2 folds in the wave-2 multi-bot schema items, the AUDIT-2 conflict
resolutions, and the AUDIT-3 activation-not-existence rewording — per AUDIT-2 C7, **this
v1.2 amendment is a precondition of signature**: signing v1.1 with the memos' schema
items still memo-text would re-create finding A2 one level up. Editing rules: amendments
are version-stamped in §12; never silently rewritten.

**Version:** 1.2 (SIGNED 2026-08-25)
**Donor:** wayland-core 0.13.0 `crates/wcore-memory` (vendored, tracked:
`resources/upstreams/wcore-0.13.0` — the v1.0 `.tmp` scratch citation is void per the
WP-6 housekeeping preconditions), design-upgraded with Graphiti semantics
(design-reference only — `github.com/getzep/graphiti`) and IJFW's decision-record
fields.

---

## 1. Donor → Nano schema mapping (every table labeled)

| wcore-memory table | Nano disposition |
|---|---|
| `episodes` (session event records) | **PORT** — add `valid_from`/`valid_to` columns (new design per Graphiti semantics) AND `project` (§6.3) AND `agent_id` (§6.8 — retrieval pass 1 covers episodes, so both partition columns must exist here too) |
| `facts` (subject/predicate/object, confidence, source_episode, superseded_by) | **PORT + UPGRADE** — keep supersession chain AND add explicit `valid_from`/`valid_to`; wcore's chain-only model is the weak form. Add `source_trust`, `project`, and `agent_id` (all `NOT NULL`) |
| `decisions` | **NEW DESIGN** — IJWM/IJFW fields as columns: `summary`, `why`, `how_to_apply`, `tags`, `source_episode`; plus `source_trust`, `project`, and `agent_id`, `NOT NULL` |
| `procedures` (how-to records) | **PORT** — add `agent_id`; wcore's `created_by` provenance column stays separate (see identity-vs-provenance below) |
| `user_model` | **OMIT v1** (Desktop owns the user model; Nano's memory is task/project-scoped) |
| `working_spillover` | **PORT** (T1↔T2 bridge) |
| CDC/change-tracking tables | **OMIT** — the journal is the change log; no second one |
| retention/recall-control tables | **PORT** (see §5 policy) |
| `kg_nodes` / `kg_edges` | **PORT** — runtime-created (as in wcore), NOT in versioned SQL migrations; tables may ship unpopulated in the core milestone. Add `agent_id` to both when they are populated by the KG-BFS package (§3 pass 3) |

**The `agent_id` column (day-one, per the OWNER OVERRIDE item 2 and Lane F item 1):**
`agent_id TEXT NOT NULL DEFAULT 'main'` on every content row — episodes, facts,
decisions, procedures, and kg_nodes/kg_edges when populated. Same discipline as
`project` in §6.3: assigned at ingest from the session's agent context, immutable after
write (correction = a new superseding write, never a mutation), `NOT NULL` so no row is
ever unattributed. Single-agent Nano writes `'main'` everywhere and never notices the
column. Retrofitting identity into a live store means re-attributing every historical
row — the expensive path this column exists to avoid.

**Identity ≠ provenance (Lane F item 6):** wcore's `source`/`source_product` columns
(write-path provenance — "how was this row produced") stay exactly as donated;
`agent_id` (identity — "whose memory is this") is a SEPARATE column. Do NOT overload
`source = 'sub-agent:<n>'`-style strings as the identity carrier — wcore's permissive
`Source::parse` fallback is the cautionary tale, and the separation is what makes
per-agent surgical rollback possible after a compromise.

## 2. The T2 store

**Storage: SQLite (rusqlite, bundled) + FTS5 + sqlite-vec.** One file:
`<nano_home>/memory/memory.db`. No services, no daemons, no graph DB (Neo4j/FalkorDB
are a hard no). Graph queries = recursive CTEs / BFS over adjacency tables — sufficient
at agent scale (≤10⁵ edges, ≤2 hops; Lane L §3.1 confirms this cell empirically).
IJFW's markdown knowledge.md becomes an **export projection**, never the store of
record.

**Bi-temporal discipline:** every fact carries `valid_from`/`valid_to` (world-time,
when the fact was true) AND the row's write timestamp (system-time, when Nano learned
it). Graphiti keeps both; wcore keeps neither cleanly; Nano keeps both. Queries default
to currently-valid facts; history is reachable, never deleted.

**Contradiction:** wcore's deterministic resolver on the write path
(Supersede / KeepExisting / Coexist, 1.2× recency bias — port `contradiction.rs`
semantics), UPGRADED to be tier-aware per §6.2: trust tier outranks recency. The
conflict domain is defined in §6.2 and includes `agent_id`. LLM-based invalidation
(Graphiti-style) is a committed program package (§9): the deterministic resolver is the
only invalidation mechanism until that package lands, and any LLM invalidation pass
ships under the §6.7 write rule — it proposes at ModelInference tier, so it can never
outrank User/ToolOutput truth (§6.2 makes that structural, not conventional).

## 3. Retrieval spec (the exact passes — no collapsing)

Every `RetrieveQuery` carries a mandatory `project` field (§6.3) AND a mandatory
`agent_scope` field (below). Both partition predicates are WHERE clauses inside passes
1–2, never post-filters.

1. FTS5 BM25 over facts/episodes text, **project- and agent-filtered** (the partition
   predicates live inside this pass).
2. sqlite-vec KNN over embeddings (dimension-aware; legacy cosine fallback for
   mixed-dimension rows), **project- and agent-filtered** (same rule).
3. KG traversal: BFS depth 1–2 from entity hits — **COMMITTED, EVIDENCE-GATED
   (activation, not existence)** per the OWNER OVERRIDE and AUDIT-3, verbatim:

   > The KG-BFS leg is committed work: build it thin (Lane L Imp. 3's spec), wire it
   > behind the retrieval config, measure it against the extended recall fixture in CI.
   > If it beats BM25+KNN it activates as the third RRF leg; if it loses, it ships
   > built, measured, dark, and documented — the negative result is the deliverable.
   > What may not happen is skipping the build.

   Build-thin spec (Lane L Imp. 3): entity nodes extracted deterministically from fact
   subjects/objects, typed edges from predicates, BFS depth ≤2 with a hard token budget
   (**≤7k tokens** total retrieved, ~5k target, matching where production systems
   cluster), RRF k=60 as specced. No community detection, no LLM entity resolution on the hot path, no
   ontology beyond prescribed types. Depth ≤2 and the token budget are **contract-level
   invariants**, same status as the §11 recall bar, testable in CI (Lane L Imp. 5 —
   retrieved-subgraph volume grows exponentially while accuracy degrades past 1–2
   hops). Before the leg may ACTIVATE, the mem-sec pack gains a relation-level
   poisoning card (Lane L Imp. 6 — GragPoison: one poisoned relation misleads many
   multi-hop queries; passage-level defenses do not transfer). The leg's real value is
   traversal-shaped provenance, not recall points.
4. RRF fusion, k=60, over the active legs (two until pass 3 activates).
5. Trust-tier down-weighting (§6.4) applied to fused scores.
6. Session-diversity limits (one source episode may not dominate results).
7. Privacy/retention filter (see §5) applied BEFORE fusion output is assembled.
8. Token trimming to the caller's budget.
9. Provenance: every returned item carries source_episode + confidence +
   valid-window + `source_trust` + `project` + `agent_id` (Lane F item 9 — "why is
   this in my context" attributes bot origin; zero schema cost, required for the
   cross-bot debugging story). Facts path and episodes path stay SEPARATE (wcore keeps
   them distinct; the facts path is cosine-only over embeddings — do not merge).

**Agent scope (Lane F item 3, as amended by AUDIT-2):** `agent_scope` is
`Own` (default — the querying agent's own `agent_id`) | `OwnAndProject` (own rows plus
the project namespace written by `main`) | an explicit agent-id list. The predicate
ANDs with the §6.3 project predicate inside passes 1–2, and the assembled-output
assertion (§11) covers it identically — the MEM-SEC-6 card (§6.5) is the executable
proof. In the core milestone with a single agent this is a no-op filter on `'main'`.
The explicit-list variant is the **schema door** for future cross-bot reads; no
cross-bot read path ships until the A2A package exists (AUDIT-2 C6) — the door stays
closed, not absent.

**Embedders:** hashed-local 384-dim is the ONLY backend in the core milestone (free,
private, zero deps, deterministic — the acceptance bar in §11 depends on that
determinism). The hosted/Flux embedding backend is a committed program package (§9),
re-gated on a confirmed Flux endpoint. The embedder stays behind `trait Embedder` so
the backend is swappable. Key handling per AGENTS.md.

## 4. Durability contract (the journal is the authority)

- **SQLite is a rebuildable INDEX, not state.** Every memory write lands in the session
  journal FIRST (new `Op::MemoryWrite` family — additive, serde-defaulted, replay
  context-neutral). The op payload records `source_trust`, `project`, AND `agent_id`
  for every row written, so journal replay reconstructs tiers, partitions, and
  attribution bit-for-bit (this is the laundering defense of §6.5, card MEM-SEC-4,
  extended to bot attribution per Lane F item 2). Corruption recovery: drop the DB,
  rebuild from journals. That ordering is the entire answer to journal↔DB consistency.
- **Location:** `<nano_home>/memory/memory.db`; per-platform canonical nano_home. **Refuse
  network filesystems** (SQLite + NFS = corruption) — typed refusal at open.
- **Single writer:** the S3 lock machinery (session ownership) applies; the DB opens
  under the session's write lock. A second process gets the typed contention error,
  never a silent second writer.
- **Concurrency model (AUDIT-2 M1, stated once here):** N agents share ONE
  `memory.db` under the single-writer lock. Multi-agent day-one means **sequential
  activations**: the fork-chain discipline (one live activation per `agent_id` at a
  time, a second concurrent spawn gets a typed `AgentBusy` refusal) means no two
  runtimes ever write one agent's namespace concurrently, and cross-agent writes
  serialize on the session lock. Concurrent-writer multi-bot is NOT a v1 claim; if a
  future package wants it, the lock story is reopened then, deliberately.
- **Migrations:** `schema_version` table; additive migrations only within a major
  version; destructive migration = export to journal-replayable form first, then
  rebuild. Rollback = drop + rebuild from journal.
- **Retention caps:** per-tier row caps + byte caps (defaults: 10k episodes, 50k facts,
  256 MiB DB) with eviction order oldest-invalidated-first, then lowest-confidence.
  Caps are policy (§5), never silent. **Accounting is keyed per `(project, agent_id)`
  from day one** (Lane F item 10): the schema needs nothing beyond the columns, but the
  eviction counters are per-agent-per-project now, so a future busy bot cannot evict
  another bot's memory under the shared caps.

## 5. MemoryPolicy — the frozen interface (P-PROF v2 consumes this)

```rust
pub struct MemoryPolicy {
    pub enabled: bool,                 // master switch; false = no reads, no writes
    pub write: WriteScope,             // Off | SessionOnly | SessionAndProject
    pub read_scope: ReadScope,         // Session | SessionAndProject  (Global REMOVED — §6.3)
    pub retention: RetentionCaps,      // §4 caps
    pub embedding_backend: EmbedderChoice, // HashedLocal only in the core milestone (hosted backend: committed package — §3, §9)
    pub deletion: DeletionRule,        // Never (invalidate-only, default) | HardDelete (explicit op only)
    pub min_tier: TrustTier,           // retrieval floor — §6.4; default ModelInference (include all)
}
```

Rules: profiles may only TIGHTEN a MemoryPolicy (a profile can disable writes; it can
never widen read scope past the launch ceiling); the resolved policy is journaled at
session start (like the resolved profile); invalidation never deletes history
(`valid_to` is set, the row stays); unknown `read_scope`/`source_trust`/`agent_scope`
values in config or journal are typed parse errors, never silently coerced (card
MEM-SEC-5).

**Agent validation (Lane F item 8):** at session start the session's `agent_id` is
validated against the configured agent registry (`$NANO_HOME/agents/*.agent.toml`,
config-file registry per the profiles precedent — NO registry table in the store).
`main` is an implicit configured agent, so validation has no special case. An unknown
id is a typed error, fail-closed. This doubles as the shared-namespace killer (§6.8):
a write naming an unconfigured id can never land.

## 6. Trust tiers and partitioning (WP-6 security preconditions — Judges 3, 1, 5)

This section is a Phase-0 precondition of the WP-6 gate, not a Phase-1 feature: no
persistent memory ships without it. The OWNER OVERRIDE reaffirms this: "no deferrals"
applies to capability, not to the safety machinery.

### 6.1 `source_trust` — the field

Every facts/decisions row carries
`source_trust TEXT NOT NULL CHECK (source_trust IN ('User','ToolOutput','ModelInference'))`.
Assignment is at ingest, by write-path origin, and is immutable after write (correction
= a new superseding write, never a mutation):

- **User** — operator-originated input: user turn text, an explicit user-invoked
  `memory store` verb.
- **ToolOutput** — content emitted by tool executions: file contents, command output,
  MCP results.
- **ModelInference** — anything the model produced without a direct User/ToolOutput
  source for that specific claim: extractions, summaries, inferred facts — **including
  every model-initiated write**, which is force-capped here by §6.7's mediation point.
- Ambiguous origin resolves to the LOWEST applicable tier (ModelInference).
- **No laundering:** any LLM-mediated transformation caps the output tier at
  ModelInference regardless of input tier. Deterministic copies (journal replay,
  export projection, DB rebuild) preserve the tier bit-for-bit. The same rule binds
  identity: replay/rebuild preserves `agent_id` bit-for-bit (§4, Lane F item 2).

### 6.2 Tier-aware resolver — the exact rule

Tier ranks: User=3, ToolOutput=2, ModelInference=1. **The conflict domain is
`(project, agent_id, subject, predicate)`** (Lane F item 4): two agents MAY
legitimately hold contradictory currently-valid facts; supersession and the tier-aware
rule apply WITHIN one agent's namespace only and never cross agents. When a new row
conflicts with a currently-valid row in the same conflict domain (incompatible object):

- `rank(new) > rank(old)` → **Supersede**, regardless of timestamps.
- `rank(new) == rank(old)` → the wcore rule, pinned to the donor's actual algorithm
  (`contradiction.rs`; AUDIT-5 A1 — the earlier "recency" English misdescribed it):
  **the 1.2× test is a CONFIDENCE comparison and timestamps play no role in the
  verdict** — compute `adjusted = new.confidence × 1.2`; Supersede iff
  `adjusted > existing.confidence`; a near-tie (`existing.confidence − adjusted < 0.1`,
  exact ties excepted below) **Coexists** with the new row stamped at ×0.8 confidence;
  otherwise KeepExisting — with one deliberate override of the donor: an exact
  adjusted-confidence tie resolves to KeepExisting (first-written wins,
  deterministic), not to the donor's tie→Coexist fall-through. MEM-SEC-1/2 fixtures
  therefore seed explicit confidences; their "newer timestamp" seed text is
  incidental, not the mechanism.
- `rank(new) < rank(old)` → **KeepExisting, always.** A lower-trust row can never
  supersede a higher-trust row on recency or anything else. The row is still stored —
  history is preserved (§4) — it simply never supersedes.

Non-conflicting rows Coexist as in wcore. Every resolver verdict is journaled with the
op outcome.

### 6.3 `project` — partitioning and the removal of ReadScope::Global

Every facts/decisions/episodes row carries `project TEXT NOT NULL`. Every
`RetrieveQuery` carries a mandatory `project` field. Enforcement is at two points: the
partition predicate is a WHERE clause inside passes 1–2 (pre-fusion), AND the assembled
output is asserted to contain only the query's project before return (§11 makes the
assertion executable). The `agent_scope` predicate (§3) follows the identical
two-point enforcement discipline, and §11's output assertion covers both.

**`ReadScope::Global` is REMOVED from v1 — full removal, not opt-in.** Justification:
profiles may only tighten policy, so a v1 Global would be an un-widen-able ceiling that
no later profile could fence; any cross-project read would bypass the partition
predicate that §6.3 and the MEM-SEC-3 card exist to prove. The enum has two variants
(`Session | SessionAndProject`); config or journal naming `Global` fails with a typed
parse error (§5). Cross-project retrieval is a committed program package (§9,
P-XPROJ): it returns ONLY as an explicit, logged, per-query opt-in, built last, after
the partition machinery has been gate-carded in anger.

### 6.4 Retrieval down-weighting — the exact rule

After RRF fusion (pass 4), every fused score is multiplied by its tier weight:
**User ×1.0, ToolOutput ×0.8, ModelInference ×0.5.** Weights are contract constants —
not profile-configurable. The multiplication happens before session-diversity
limits and token trimming so untrusted items lose rank, not just budget.
`MemoryPolicy.min_tier` additionally EXCLUDES tiers below the floor from retrieval
entirely (a profile may set it to User and read nothing else); default includes all
tiers.

### 6.5 Memory-write adversarial gate-card pack (`mem-sec`)

A gate-card pack `mem-sec` runs through the existing WP-3 verify CLI under the WP-4
gate-card machinery. The pack MUST pass on all 7 CI legs (Windows ARM64 named — the
WP-6 spike flags it as the predicted failure point). The milestone fails if any card
fails. Cards:

| Card | Seed state | Adversarial action | Invariant (pass condition) |
|---|---|---|---|
| MEM-SEC-1 poisoned supersession | User-tier fact F1 (`deploy_target = staging`), currently valid | Write ToolOutput-tier F2 (`deploy_target = prod`) with a NEWER timestamp | Resolver returns KeepExisting per §6.2; F1 stays currently-valid; F2 stored non-superseding; retrieval for `deploy_target` returns F1 above F2 |
| MEM-SEC-2 same-tier control | ToolOutput-tier G1, currently valid | Write ToolOutput-tier G2 conflicting, newer, passing the 1.2× test | Resolver Supersedes — proves the §6.2 same-tier path still fires (guards against an over-tightened resolver that never supersedes) |
| MEM-SEC-3 cross-project leak | 30 facts in project A, 30 in project B | Query with `project = B` across passes 1, 2, and fused output | Zero project-A rows at every checkpoint: pass-1 hits, pass-2 hits, and final assembled output |
| MEM-SEC-4 extraction laundering | ModelInference-tier fact H1 under `agent_id = 'bot-a'` | (a) LLM-extract a new row from a ToolOutput episode; (b) replay the journal; (c) rebuild the DB from journals | (a) extracted row is ModelInference (§6.1 cap); (b,c) H1's tier AND `agent_id` are unchanged after replay/rebuild — tier and attribution survive the journal round-trip bit-for-bit (extended to bot attribution per Lane F item 2) |
| MEM-SEC-5 removed-scope escape | Config and journal fixtures naming `read_scope = "Global"`, an unknown `source_trust`, an unknown `agent_scope`, or an unconfigured `agent_id` | Parse them; attempt session start and a write under the unconfigured id | Typed parse error in all parse cases; session start and the write both fail closed with a typed error on the unconfigured id (§5, §6.8 — the shared-namespace killer is executable, not prose); no silent coercion, no default-to-widest |
| MEM-SEC-6 cross-bot leak (new in v1.2 — Lane F item 7) | 30 facts under `agent_id='bot-a'` and 30 under `agent_id='bot-b'` in ONE project | Query as bot-b with default scope (`Own`) across passes 1, 2, and fused output | Zero bot-a rows at every checkpoint: pass-1 hits, pass-2 hits, and final assembled output (mirrors MEM-SEC-3's shape; reuses its harness) |

A seventh, relation-level poisoning card is REQUIRED before the §3 pass-3 KG-BFS leg
may activate (Lane L Imp. 6): poison a single kg_edge, then assert the BFS leg's
fan-out doesn't let it dominate fused results (levers: §6.4 tier weights +
session-diversity limits). Growing this pack — including MEM-SEC-6 and the future
relation card — triggers the §6.6 owner-review rule for `gates/**`.

Fixture rows live with the pack; changing them is a `gates/**` change (§6.6).

### 6.6 `gates/**` review rule (registry self-re-seal defense)

Every change under `gates/**` — gate-card registry, pack definitions, expected-verdict
fixtures, including the `mem-sec` pack above — requires human review per the repo's
CODEOWNERS: owner approval before merge, enforced by branch protection on the path.
An agent may PROPOSE gate cards; it may never approve or merge changes to the cards
that certify its own work. The CODEOWNERS entry itself is a WP-6 Phase-0 housekeeping
item; this clause binds P-MEM from contract signature onward. **The same rule extends
to shipped agent definitions and persona files** (`agents/**`) — they are prompt-code;
the category's persona incidents were unreviewed prompt changes (AUDIT-2 M3, §6.9).

### 6.7 Write authority — model proposes, host commits (new in v1.2; resolves AUDIT-2 C1)

Who may initiate a memory write, exhaustively:

- **Host-originated writes commit directly**: user-turn ingest, tool-output ingest,
  and explicit user-invoked memory verbs write on the host path with the §6.1 tier of
  their origin.
- **Model-originated writes are PROPOSALS ONLY.** The model's memory surface is
  read-only recall tools plus a `memory_propose` tool. The host commits proposals at a
  single mediation point that: (a) **force-caps `source_trust` at ModelInference** —
  a proposal can never carry a higher tier, per §6.1's no-laundering rule; (b) applies
  **deterministic-only write-time screening** — redaction + re-scan, reusing the
  existing memory write path; NO LLM screener (§9 anti-scope; AUDIT-2 C8 — "sanitize"
  means a named deterministic pass, never an LLM-in-the-loop); (c) **journals
  `Op::MemoryWrite`** with `agent_id`, `source_trust`, and `project`; and (d) emits a
  **user-visible receipt** — a journaled "memory updated for ⟨agent_id⟩" receipt line
  (Lane I item 4, triple-confirmed as category standard by Lane N: visible writes kill
  silent-rot at the UX layer).
- **Procedure extraction (learn-from-doing) follows the same rule** (owner decision,
  2026-08-26): extraction from journals into the T2 `procedures` table is
  model-proposes/host-commits, trust-tiered, with the visible receipt. No separate
  write path for procedures.
- Agent/child memory tools are **in-process tools, never MCP exposure** — §9's ban on
  MCP exposure of memory is untouched (AUDIT-3 consistency note: a future reader must
  not collapse the two).

### 6.8 Identity surfaces — one id, one grammar (new in v1.2; AUDIT-2 C4/C5/M5)

- **`agent_id`:** one flat string, PROFILES strict name grammar
  (`[a-z0-9][a-z0-9-]{0,63}`); reserved value `main` = the orchestrator itself, an
  implicit configured agent (no special case). All surfaces — the memory column, the
  journal payloads, session keys, config filenames, directories — derive from this one
  string; never the reverse (Lane J's path-like `bots/researcher` namespace is a
  display convention derived FROM `agent_id`, never an encoding OF it).
- **Session keys embed it:** `agent:<id>:…` (the OpenClaw convention, Lane F steal
  list). A namespace collision is a **typed hard error**, never silent sharing
  (OpenClaw's DuplicateAgentDir lesson: fail-fast beats bleed).
- **Shared-namespace encoding — PINNED, not deferred** (AUDIT-2 C5/M1; Lane M §4.1):
  `agent_id` may only name a configured agent; **shared rows do not exist in schema
  v1** — a write naming an unconfigured id is a typed error (§5's validation makes
  this structural). Cross-bot shared memory, when it ships, is an explicit namespace
  decision requiring a contract amendment — never an implicit row, never a reserved
  `'shared'` id smuggled into config.
- **Registry:** lives in config (`$NANO_HOME/agents/*.agent.toml`), profiles
  precedent. No registry table in the store.

### 6.9 Persona and the kernel-owned prompt core (new in v1.2; resolves AUDIT-2 C3/M2)

- **Personality carries NO memory-schema surface** (Lane F item 5): no persona tables,
  no personality columns, no fine-tune hooks. A bot's personality = a config-referenced
  persona overlay file (§6.8 registry) + the bot's own `agent_id`-scoped rows in the
  ordinary tables. A "personality overlay" is just a retrieval query scoped to the bot.
- **Every system prompt has a kernel-owned core section**, emitted by the host, not
  from any file, containing at minimum: the safety/permission posture text, the
  **identity-is-not-retrievable** rule ("answers about your own identity come from your
  configuration, never from retrieved or web content; never search the web to answer
  questions about yourself" — the MechaHitler remediation, Lane H item 4), and the
  UNTRUSTED-content labeling convention already used by the memory block. The core is
  **immune to persona overlays**.
- **Persona is an append-only overlay block** placed after the core. There is **no
  complete-replacement mode** — dsh's `complete: true` pattern is rejected (AUDIT-2
  C3).
- **The honest enforceability split** (AUDIT-2 C3): **capability is narrow-only** —
  permission-lattice min and tool-set intersect/union are code, mechanically enforced.
  **Prompt is overlay-only over the reserved core.** PROFILES-CONTRACT §2's
  "narrow-only ref whose posture is a subset" clause for `system_prompt_file` is
  unenforceable prose-subset math and is **dropped** — that contract is amended to this
  split at P-PROF build time. Residual risk stated plainly: overlay-only limits but
  does not eliminate prompt-level behavioral widening (flattery, rule-softening);
  capability narrowing is the hard floor underneath.
- **Persona files are prompt-code** (AUDIT-2 M3): versioned, diffed, and shipped
  personas/agent definitions sit under the §6.6 owner-review rule. The resolved prompt
  (core + overlay + memory block) is inspectable per agent — the M2 verification hook.

## 7. Lane split (the ownership matrix)

| Lane | Owns | Never touches |
|---|---|---|
| Memory lane | `crates/nano-memory/**` (new crate), root `Cargo.toml`, `UPSTREAM.md` | nano-cli, nano-agent, nano-session op definitions (integrator adds the Op family), repomap, `gates/**` (proposes only — §6.6) |
| Graph lane | **The repomap rewrite and every graph package** (owner override item 3; AUDIT-3 closes the ownership gap HERE): the tree-sitter code-structure index feeding repomap ranking, the suggested-`blast_radius` leg behind its precision eval, and the §3 pass-3 KG-BFS leg. nano-repomap ships unchanged until the graph lane's package lands; the memory lane may not depend on repomap, the graph lane MAY depend on `nano-memory` types crate-level. | nano-memory internals, nano-cli, nano-agent |
| Integrator | `Op::MemoryWrite*` in nano-session (additive), wiring seams in nano-agent/nano-cli, error kinds | the memory lane's crate |

**Shared types live HERE, not in either lane's head:** the skeletons below (with
`MemoryPolicy`, §5) are the pinned interface — all lanes import them from
`nano-memory`'s public API. They pin field presence and semantics only (AUDIT-5 A2 —
written into the contract so no lane invents the de-facto interface unreviewed);
private fields, constructors, and helpers are the memory lane's design space.

```rust
pub enum TrustTier { User, ToolOutput, ModelInference } // §6.1; ranks 3/2/1 (§6.2)

pub struct Fact {                            // §1 PORT + UPGRADE
    pub id: String,                          // uuid v7 (donor schema/v1.sql types)
    pub subject: String, pub predicate: String, pub object: String,
    pub confidence: f64,                     // the §6.2 resolver's input
    pub source_trust: TrustTier,             // §6.1, NOT NULL, immutable
    pub source_episode: String,
    pub project: String,                     // §6.3, NOT NULL
    pub agent_id: String,                    // §1, NOT NULL DEFAULT 'main', immutable
    pub valid_from: i64, pub valid_to: Option<i64>, // §2 bi-temporal, unix seconds
    pub superseded_by: Option<String>,       // wcore supersession chain kept (§1)
}

pub struct Episode {                         // §1 PORT
    pub id: String,
    // donor event-record fields unchanged (source/source_product provenance —
    // §1 identity ≠ provenance), plus:
    pub project: String, pub agent_id: String,      // §6.3, §1 — pass 1 covers episodes
    pub valid_from: i64, pub valid_to: Option<i64>, // §1 Graphiti-semantics add
}

pub struct Decision {                        // §1 NEW DESIGN (IJFW fields as columns)
    pub id: String,
    pub summary: String, pub why: String, pub how_to_apply: String,
    pub tags: Vec<String>,
    pub source_episode: String,
    pub source_trust: TrustTier, pub project: String, pub agent_id: String,
    pub valid_from: i64, pub valid_to: Option<i64>,
}

pub struct KgNode { /* §1 PORT — runtime-created, may ship unpopulated; `agent_id` added when populated by the KG-BFS package (§3 pass 3) */ }
pub struct KgEdge { /* same discipline as KgNode */ }

pub enum AgentScope {                        // §3 — `Own` is the default
    Own, OwnAndProject, Explicit(Vec<String>),
}

pub struct RetrieveQuery {                   // §3
    pub text: String,
    pub project: String,                     // mandatory — §6.3
    pub agent_scope: AgentScope,             // mandatory — §3
    pub token_budget: usize,                 // pass 8 trims to the caller's budget
}

pub struct RetrieveHit {                     // §3 pass 9 provenance, post-fusion
    pub id: String,
    pub score: f64,                          // post-RRF (pass 4), post-§6.4 tier weight
    pub source_episode: String,
    pub confidence: f64,
    pub valid_from: i64, pub valid_to: Option<i64>,
    pub source_trust: TrustTier, pub project: String, pub agent_id: String,
}

pub trait Embedder {                         // §3 — hashed-local 384-dim is the ONLY
    fn embed(&self, text: &str) -> Vec<f32>; // core-milestone backend; deterministic
}
```

**P-MEM-1 seam boundaries (AUDIT-5 M8/A5/A6 — what the core WP may touch):** the WP
may touch `crates/nano-memory/**` (the new crate), the `Op::MemoryWrite*` family in
nano-session's `op.rs` (additive only), and the §6.7 mediation wiring in nano-agent.
Everything else is another WP's lane. Two boundary calls, pinned:

- §5's agent-registry validation is **P-BOT-5a's deliverable, not P-MEM-1's** (AUDIT-5
  M8): at P-MEM-1 the configured-agent set is exactly `{main}` (the implicit
  configured agent), so validation trivially passes and the fail-closed path (§5,
  MEM-SEC-5) is exercised by test fixtures, not by a registry implementation.
  Building the file-based registry now is scope creep into P-BOT-5a — the P-MEM-1
  tripwire names this shape.
- `Op::MemoryWrite*` payloads carry **full row content** (AUDIT-5 A5) — a deliberate,
  REQUIRED deviation from the repo's digest-only journal convention (tool outputs
  journal digests only). The §11.2 durability bar (drop the DB, rebuild from
  journals, query-equivalent) is unreachable from digests, and memory rows are not
  the secret payloads that convention exists to protect.

## 8. Memory-aware compaction

**Committed program package (P-CONS)** — OWNER OVERRIDE item 1: nothing is deferred;
the package ships inside the program, sequenced per `../NANO-PROGRAM-PLAN.md`. Until
that package lands, the consolidator does NOT extract facts/decisions from the
compacted span; compaction stays summary-only, no extraction model call, no T2 writes
from compaction. The only core-milestone artifact is the flag itself:
`memory.compaction_extraction`, default OFF; setting it ON before P-CONS is a typed
config error (the machinery behind it does not exist yet). When P-CONS builds:
extraction follows the §6.7 write rule (model-proposes/host-commits, ModelInference
cap, visible receipt), the extraction-model cost question (§10.1) is decided in the
package's design phase, and the **F-45 conversation-retention policy question is a
design input to the package** (OWNER OVERRIDE item 5 — F-45 is closed as
measured/accepted-residual; its retention question moves here, not into a deferral).

## 9. Anti-scope (do NOT build)

Neo4j/FalkorDB/any graph service · community detection / graph summarization ·
LSP/compiler-grade code resolution (the graph lane is tree-sitter only; if precise
blast radius is ever required, LSP/SCIP-grade is a separate named package — never
tree-sitter name-matching asserted as truth) · LLM-in-the-loop ingest on the write
path (§6.7's screening is deterministic-only) · a general ontology system (prescribed
types only) · committing generated graphs to git · user-model (Desktop owns it) · any
MCP exposure of memory (removed from mcp-serve v1 by the 0.2 audit — stays out until
the policy boundary is proven; §6.7's agent tools are in-process, not MCP) · bot-to-bot
messaging day-one (AUDIT-2 C6 — schema door open via §3's explicit agent-id list, the
feature is a named later package) · teach-by-demonstration screen capture (Lane N
reject — captured procedures must be versioned diffable artifacts, which is a
skill-authoring feature, not a recording feature) · P-MOD/modules beyond the
manifest-gap closure package · CUA 150% leg (owner hardware) · image-gen.

**Committed program packages (OWNER OVERRIDE 2026-08-23, item 1: no deferrals — these
are sequenced work packages in `../NANO-PROGRAM-PLAN.md`, each shipping with the same
verified/gate-carded bar as the core; sequencing exists because later packages
genuinely build on earlier ones, not because anything is shelved):**

- Code-structure index + repomap rewrite; suggested `blast_radius` (P-GRAPH-1/2 —
  graph lane, §7).
- KG-BFS third RRF leg (§3 pass 3) — activation-gated per the verbatim clause in §3.
- Hosted embeddings backend (§3) — re-gated on a confirmed Flux endpoint.
- Memory-aware compaction extraction (§8, P-CONS).
- LLM offline invalidation pass (§2) — ships under the §6.7 write rule; the
  deterministic tier-aware resolver remains the only invalidation mechanism until
  then, and the pass can only ever propose at ModelInference tier.
- `ReadScope::Global` / cross-project reads (§6.3, P-XPROJ) — explicit, logged,
  per-query opt-in; built LAST, after the partition machinery is gate-carded in anger.

## 10. Open questions and recorded owner calls — none block the contract

1. Extraction LLM for consolidation: session model (Flux completions) vs cheap local
   path — 10× cost difference; owner call. **Decided in P-CONS's design phase** (§8).
2. ~~Flux embeddings endpoint existence~~ — **RESOLVED for the core milestone: cut.**
   Reopens as the hosted-embeddings package's P-ASK item (§9).
3. Donor license check for wcore-memory port (Apache-2.0 + NOTICE expected — verify at
   build time; UPSTREAM.md entries mandatory; donor vendored at
   `resources/upstreams/wcore-0.13.0` per WP-6 Phase 0).
4. Code-graph scope: whole-workspace vs repo-root (recommend: repo-root, matching the
   existing repomap discipline). Decided in P-GRAPH-1.
5. **Continuity substrate for persistent-agent resume (Q2) — DECIDED 2026-08-25:
   memory-primary, for everything.** Fresh context each activation; continuity loads
   via scoped memory recall; journals chain for audit only (transcript-replay is the
   audit/fallback path, not a second verified default). Recorded in
   `../NANO-PROGRAM-PLAN.md` §4. Consequence for this contract: the §11 bar-1 recall
   fixture is load-bearing for the product experience — memory quality IS the
   continuity substrate.
6. **Agent-id recycling (Q7) — DECIDED 2026-08-25: never-recycle.** Re-creating a
   retired id is a typed error, forever — one identity per id, ever (forensically
   clean attribution). Tombstone store = a `retired` marker file in `agents/`
   (config-side, no DB table — §6.8's registry discipline). Recorded in
   `../NANO-PROGRAM-PLAN.md` §4.

## 11. Acceptance criteria

All bars run through the WP-3 verify CLI and gate the milestone like any other card.

1. **Retrieval quality.** Fixture set `memory-retrieval-recall-v1` (authored and
   committed as part of P-MEM-1 per this spec — it does not exist at signature time;
   AUDIT-5 A3): 50 facts + 10 decisions split 30/30 across two
   projects, 20 fixed queries with labeled relevant row ids. Pass bar, hashed-local
   embedder (deterministic → the bar is CI-stable): **aggregate recall@10 ≥ 0.90**,
   and **zero cross-project rows in any query's output** (the §6.3 output assertion,
   measured — not just unit-tested). **v1.2 extensions:** the fixture gains a two-agent
   split so the same zero-leak assertion covers `agent_scope` (the MEM-SEC-6 shape,
   measured on the recall harness); and the fixture is EXTENDED
   (`memory-retrieval-recall-v1` extended, versioned alongside) as the measuring
   instrument for the §3 pass-3 activation gate — the KG-BFS leg activates iff it
   beats BM25+KNN-only recall@10 on the extended fixture, otherwise it ships dark with
   the measured negative result documented (§3's verbatim clause).
   **Fixture pins (AUDIT-5 A3/L7):** the fixture lives at
   `gates/fixtures/memory-retrieval-recall-v1/**` — a §6.6-reviewed `gates/**`
   change, never `crates/nano-memory/tests/` (the bar the agent sets for itself
   requires human review); format is JSON — the row set, the 20 queries, and the
   labeled relevant row ids per query; the two-agent split DUPLICATES the same 60
   rows across two `agent_id`s (a re-attribution, not 60 more rows). Relevance
   labels are assigned by human-readable relevance, NEVER by retrieval output, and
   the fixture + labels land in a SEPARATE commit/PR from the retrieval
   implementation — no single change both writes and grades the bar (the
   anti-self-grading rule; §6.6 review applies to both PRs).
   **Fixture-label honesty (AUDIT-5 §6 item 4):** hashed-local bag-of-words KNN is a
   weak retriever; honestly authored (paraphrase-heavy) labels may put 0.90 out of
   reach. If the build produces evidence the bar is unreachable, that evidence
   triggers a CONTRACT AMENDMENT discussion — never fixture-tuning to make the bar
   pass (three-strikes protection: tuning the fixture to pass IS the failure class).
2. **Durability.** Fault-injection scenario: SIGKILL the writer at the injected fault
   point between journal append and DB commit, then DELETE `memory.db`, then rebuild
   from journals. Pass bar: the rebuilt DB is **query-equivalent** to a no-kill control
   run — identical hit-id lists in identical order for every
   `memory-retrieval-recall-v1` query, and an identical currently-valid fact set
   (same ids, same `valid_from`/`valid_to`, same `source_trust`, same `project`,
   **same `agent_id`** — attribution is part of query-equivalence, Lane F item 2).
3. **Adversarial pack.** The `mem-sec` pack — **six cards** including MEM-SEC-6
   (cross-bot leak, §6.5) — green on all 7 CI legs. MEM-SEC-4's laundering invariant
   covers bot attribution: `agent_id` survives the journal round-trip bit-for-bit.
4. **Write-authority.** No code path commits a model-originated row except through
   the §6.7 mediation point: force-capped `ModelInference`, deterministic screening,
   journaled with `agent_id`, receipted. Executable form: a gate test attempts a
   model-tier commit through every exposed write surface (recall tools, propose tool,
   direct store handle) and asserts only the mediated path lands — everything else
   fails closed.

## 12. Changelog (amendments are version-stamped here; never silently rewritten)

**v1.2 — 2026-08-26.** Sources: WP-6 OWNER OVERRIDE (2026-08-23 — no deferrals;
multi-bot is a day-one schema requirement; graph engineering in-scope); Lane F
`../MEMO-multi-bot-memory.md` items 1–10; AUDIT-2 (`bots/AUDIT-2-design.md`) conflict
resolutions C1–C8 and gaps M1–M5; AUDIT-3 (`bots/AUDIT-3-wave2.md`)
activation-not-existence rewording and repomap-ownership gap; Lane L
(`bots/MEMO-graph-engineering.md`) KG-BFS discipline; Lane M
(`bots/DESIGN-persistent-subagents.md`) concurrency model and shared-namespace
encoding; Lane N (`bots/MEMO-grokbot-hermes-bots.md`) visible-write confirmation and
routine-activation continuity ruling; owner decisions 2026-08-26 (learn-from-doing;
blast_radius labeling); AUDIT-4 (`bots/AUDIT-4-contract-v12.md`) pre-signature fixes
FIX-1–FIX-3; AUDIT-5 (`bots/AUDIT-5-buildability.md`) pre-launch pins. Every
amendment below traces to one of those:

- §1: `agent_id TEXT NOT NULL DEFAULT 'main'` on episodes/facts/decisions/procedures
  (and kg tables when populated), ingest-assigned, immutable. [OWNER OVERRIDE item 2;
  Lane F item 1]
- §1: identity-vs-provenance split — `source`/`source_product` ≠ `agent_id`, separate
  columns. [Lane F item 6]
- §2: conflict domain now includes `agent_id`; LLM invalidation restated as a
  committed package under the §6.7 write rule. [OWNER OVERRIDE item 1]
- §3: `RetrieveQuery.agent_scope` (Own | OwnAndProject | explicit list) — predicate
  ANDs with the §6.3 project predicate inside passes 1–2; assembled-output assertion
  covers it; explicit-list variant is the schema door for cross-bot reads, closed
  until A2A exists. [Lane F item 3, as AMENDED by AUDIT-2; AUDIT-2 C6]
- §3 pass 3: KG-BFS leg reinstated as COMMITTED, EVIDENCE-GATED — the AUDIT-3
  verbatim activation-not-existence clause; build-thin spec, depth ≤2 and token
  budget as contract invariants; relation-level mem-sec card required before
  activation. [AUDIT-3; Lane L Imp. 3/5/6; OWNER OVERRIDE items 1, 3]
- §3 pass 9: `agent_id` added to recall provenance output. [Lane F item 9]
- §4: `Op::MemoryWrite*` payloads record `agent_id` alongside `source_trust` and
  `project` — laundering defense extended to bot attribution. [Lane F item 2]
- §4: retention-cap accounting keyed per `(project, agent_id)` from day one.
  [Lane F item 10]
- §4: concurrency model stated — one DB, single-writer lock, sequential activations,
  `AgentBusy`; concurrent-writer multi-bot is not a v1 claim. [AUDIT-2 M1; Lane M §2.3]
- §5: agent registry validation at session start — unknown `agent_id` is a typed
  error; `main` implicit; registry in config, no table. [Lane F item 8]
- §6.2: resolver conflict domain pinned as `(project, agent_id, subject, predicate)`.
  [Lane F item 4]
- §6.5: MEM-SEC-6 (cross-bot leak) added — the pack is six cards; growth triggers
  §6.6 owner review; MEM-SEC-4 extended to attribution round-trip; MEM-SEC-5 extended
  to unknown `agent_scope`. [Lane F items 2, 7; AUDIT-2 C7]
- §6.6: review rule extended to shipped `agents/**` (definitions + personas are
  prompt-code). [AUDIT-2 M3]
- §6.7 (new): write authority — model-proposes/host-commits with the ModelInference
  force-cap, deterministic-only screening, journaled commit, visible receipt;
  procedure extraction under the same rule. [AUDIT-2 C1 resolution — Lane H item 3
  AMENDED, Lane I item 4 ADOPTED; AUDIT-2 C8; Lane N triple-confirmation; owner
  decision 2026-08-26]
- §6.8 (new): identity surfaces — one `agent_id` string/grammar; `agent:<id>:`
  session keys; collision hard error; shared-namespace encoding pinned (shared rows
  do not exist in schema v1). [AUDIT-2 C4, C5, M5; Lane M §4.1]
- §6.9 (new): persona — overlay-only over the kernel-owned immune core;
  identity-is-not-retrievable rule; capability-narrow-only is code, prompt-subset
  claims dropped; personas as reviewed prompt-code; resolved-prompt inspectability.
  [AUDIT-2 C3, M2, M3; Lane H item 4 AMENDED]
- §7: graph lane un-deferred and NAMED owner of the repomap rewrite plus all graph
  packages. [AUDIT-3 synthesis-risk; OWNER OVERRIDE item 3]
- §8: compaction extraction is committed package P-CONS; flag shell unchanged;
  F-45's retention question is a P-CONS design input. [OWNER OVERRIDE items 1, 5]
- §9: the v1.1 "DEFERRED to P-MEM-2" list converted to committed program packages;
  teach-by-demonstration and day-one bot-to-bot added to anti-scope. [OWNER OVERRIDE
  item 1; Lane N; AUDIT-2 C6]
- §10: owner calls Q2 (continuity substrate) and Q7 (agent-id recycling) recorded AS
  DECIDED 2026-08-25 (memory-primary, for everything; never-recycle). [Lane M open
  questions 2, 7; AUDIT-3 classification; owner decisions 2026-08-25; AUDIT-4 FIX-1]
- §11: acceptance extended — two-agent fixture split + zero-leak assertion on
  `agent_scope`; extended fixture as the KG-BFS activation instrument; `agent_id` in
  the durability query-equivalence set; new write-authority bar. [Lane F items 2, 3,
  7; AUDIT-3]
- §3 pass 3: KG token budget pinned to a CI-testable constant (≤7k tokens total
  retrieved, ~5k target). [AUDIT-4 FIX-3]
- §6.2: same-tier resolver rule pinned to the donor's actual algorithm — the 1.2×
  test is a confidence comparison (`new.confidence × 1.2 > existing.confidence`),
  timestamps play no role in the verdict, the donor's near-tie Coexist branch is
  kept (new row at ×0.8), exact tie → KeepExisting overrides the donor's
  tie→Coexist; MEM-SEC-1/2 fixtures seed explicit confidences. [AUDIT-5 A1]
- §6.5: MEM-SEC-5 extended — an unconfigured `agent_id` at session start/write is a
  typed fail-closed error exercised by the card (the §5/§6.8 shared-namespace killer
  made executable). [AUDIT-4 FIX-2]
- §7: the type skeletons written into the contract (`Fact`, `Episode`, `Decision`,
  `KgNode`, `KgEdge`, `AgentScope`, `RetrieveQuery`, `RetrieveHit`, `Embedder`,
  alongside §5's `MemoryPolicy`) — the previous "contract's Rust skeletons" citation
  referenced skeletons that were never written. [AUDIT-5 A2]
- §7: P-MEM-1 seam boundaries pinned — the WP may touch `nano-memory`, the
  nano-session `Op::MemoryWrite*` family, and nano-agent mediation wiring; §5's
  registry validation is P-BOT-5a's deliverable (configured set = `{main}` at
  P-MEM-1); `Op::MemoryWrite*` payloads carry full row content as a required
  deviation from the digest-only journal convention. [AUDIT-5 M8/A5/A6]
- §10: header retitled (open questions and recorded owner calls); Q2/Q7 recorded as
  DECIDED with the broken `§owner-calls` pointer fixed to §4. [AUDIT-4 FIX-1]
- §11.1: recall fixture pinned — authored in P-MEM-1 (not pre-existing), location
  `gates/fixtures/memory-retrieval-recall-v1/**`, JSON rows + queries + labels,
  two-agent split = the same 60 rows re-attributed, labels by human-readable
  relevance, fixture/labels in a separate commit/PR from the retrieval
  implementation (anti-self-grading); unreachable-bar evidence triggers a contract
  amendment discussion, never fixture-tuning. [AUDIT-5 A3/L7; AUDIT-5 §6 item 4]

**v1.1 — 2026-08-24.** Source: WP-6 decision gate (2026-08-23), "PROCEED to P-MEM-lite,
gated on preconditions." Every amendment below traces to the gate:

- §1, §6.1: `source_trust` field (User | ToolOutput | ModelInference) on
  facts/decisions, with ingest-time assignment, immutability, and the no-laundering
  cap. [WP-6 convergence item 3; Judges 3, 1, 5]
- §2, §6.2: deterministic contradiction resolver made tier-aware — trust tier outranks
  recency; a lower-tier row never supersedes a higher-tier row. [Judge 3]
- §3, §6.4: retrieval down-weights untrusted tiers (×1.0 / ×0.8 / ×0.5 post-RRF) and
  `MemoryPolicy.min_tier` floor added to §5. [Judge 3]
- §1, §5, §6.3: `project` column on facts/decisions/episodes; partition enforced
  inside retrieval passes 1–2 plus an assembled-output assertion;
  **`ReadScope::Global` fully removed from v1** (decision: full removal over opt-in —
  §6.3 justification). [Judge 3; independently flagged by two other judges]
- §6.5: memory-write adversarial gate-card pack `mem-sec` (five cards: poisoned
  supersession, same-tier control, cross-project leak, extraction laundering,
  removed-scope escape) on the WP-3 verify CLI / WP-4 gate-card machinery. [Judge 3]
- §6.6: `gates/**` human-review clause (CODEOWNERS-enforced) as the registry
  self-re-seal defense. [Judge 3]
- §3, §7, §8, §9: panel scope cuts applied and marked DEFERRED to P-MEM-2: code-graph
  call edges / `blast_radius` [Judge 5 + panel], KG-BFS third RRF leg [panel], hosted
  embeddings [panel], memory-aware compaction extraction (flag-gated default-OFF shell
  only) [panel; Judges 3, 4], LLM offline invalidation pass [panel]. **(Superseded by
  the OWNER OVERRIDE — see v1.2 §9: these are committed program packages, not
  deferrals.)**
- §11: acceptance criteria added — retrieval-quality bar (v1.0 had none) and
  durability bar (kill-mid-write + DB-drop + journal-rebuild ⇒ query-equivalent).
  [WP-6 convergence item 3 note; Judges 1, 3]
- §10: open questions re-scoped — Flux embeddings resolved (cut); extraction LLM and
  code-graph scope carried to the program packages.
- Header: donor citation moved from the untracked `.tmp` path to the vendored
  `resources/upstreams/wcore-0.13.0`. [WP-6 Phase-0 housekeeping]
- Tripwire adopted (Judge 4 dissent): if the core milestone is not clearly landing
  within ~5 working days of Phase 0 completion, stop, ship what exists, re-run WP-6
  with profiles leading. Recorded here so the contract carries the gate's stop rule;
  `../NANO-PROGRAM-PLAN.md` carries the per-package tripwires that inherit it.

**v1.0 — 2026-08-20.** Initial draft, unsigned.

## 13. Signature

**Owner:** SIGNED — 2026-08-25, following AUDIT-4 (fidelity: SIGN-WITH-FIXES) and
AUDIT-5 (buildability: BUILDABLE-WITH-FIXES) with all sign-blocking fixes applied
(§12 v1.2 changelog). Signature of v1.2 unlocks the memory program per the WP-6 gate
as amended by the OWNER OVERRIDE. Per AUDIT-2 C7: v1.1 was never signed.

