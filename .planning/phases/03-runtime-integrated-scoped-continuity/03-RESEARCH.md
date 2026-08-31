# Phase 3 Research: Runtime-Integrated Scoped Continuity

**Method:** independent reconnaissance agent over a fresh detached checkout of merged `origin/master` (`c10dcb9`), all claims file:line-cited; condensed here. No code was modified.

## 1. What Phase 2 actually quarantined (the surface Phase 3 reopens — selectively)

Quarantine is a typed-denial + omission + early-return pattern keyed on activation presence (`phase2_persistence_quarantined = activation.is_some()`, `crates/nano-cli/src/acp_mode.rs:1958`):

- Authenticated ACP: legacy `memory_*` tool defs omitted (`acp_mode.rs:4493-4505`) and forced calls denied `UnknownTool` before store access (`crates/nano-agent/src/memory.rs:436-468`); memory context injection gated `if active.activation.is_none()` (`acp_mode.rs:3607-3618`).
- protocol-host: always quarantined executor, no memory defs (`crates/nano-cli/src/host_mode.rs:291-294`).
- exec: no memory wiring at all; cron wrapped `CronjobExecutor::quarantined` (`crates/nano-cli/src/exec_run.rs:309-322`).
- Cron: forced-call denial (`crates/nano-agent/src/cron.rs:945-992`); ticker constructed only when not quarantined (`acp_mode.rs:2060-2064`); `phase2_cron_quarantined() -> bool { true }` is a const — no env/config can revive fire/tick (`crates/nano-cli/src/cron_fire.rs:20-22, 70-74, 232-237`).
- Hooks: session/per-turn/compaction hooks early-return or swap to `HookEngine::empty()` when activated (six call sites; `acp_mode.rs:1723-1734, 4726-4768, 5145-5160`).
- `NANO_HOOKS_ENABLED`/`NANO_CRON_ENABLED` exist only as test canaries — no source reads them (`activation_quarantine.rs:259-260`).
- Legacy `Op::MemoryWrite*`/`Cron*` fold as no-ops on replay (`crates/nano-session/src/replay.rs:639-644`; proven by `nano-session/tests/activation_legacy_replay.rs`).
- External-state oracle: seeded `memory/`, `t2/`, `cron/jobs.json`, `hooks.toml` must be byte-identical after runs (`activation_quarantine.rs:31-109`).

## 2. The legacy memory implementation (migration target)

- Tools: `memory_list`/`memory_read` always; `memory_save`/`memory_delete` behind `NANO_MEMORY_WRITE` (`crates/nano-agent/src/memory.rs:363-409`).
- Store: plain filesystem `<nano_home>/memory/*.md`, timestamped-slug filenames, caps 8k chars/entry, 50 entries, 24k injected block (`memory.rs:31-38, 66-230`). Write path redacts + re-scans, atomic tmp+rename, no-follow symlinks.
- Context seam: `prepare_memory_context` renders one `Role::System` block labeled UNTRUSTED, re-read every turn (`memory.rs:40, 239-254`).
- **There is no T2 implementation in this repo** — `t2/` is a canary path + a comment inherited from the Track-A donor. Only the `.md` store is real.
- The real T2 journal ops already exist: `Op::MemoryWriteFact/Decision/Episode/Procedure/Receipt` + `MemoryPolicyResolved` (`crates/nano-session/src/op.rs:1703-1780`) all carry `project`, `agent_id`, optional `session_id` — written only by nano-memory, which nothing runs yet.

## 3. nano-memory — what P-MEM-1 already shipped (Phase 3 does NOT rebuild this)

- API (`src/lib.rs:10-18`): `MemoryStore::{open, open_at, write_fact, write_decision, write_episode, write_procedure, retrieve, current_facts, commit_proposal}`, `rebuild_from_journals`, `resolve_contradiction`, `HashedEmbedder`, `register_sqlite_vec`.
- Schema (`src/schema.rs`): `episodes`, `facts` (+ conflict index on `(project,agent_id,subject,predicate,valid_to)`), `decisions`, `procedures`, `working_spillover`, `retention_control`, **`kg_nodes`/`kg_edges` present but no writer exists**; FTS5 `memory_fts`; vec0 `memory_vec` (384-dim, partition keys project/agent_id/session_id). Every content table has `agent_id DEFAULT 'main'` + `session_id`.
- Retrieval (`src/store.rs:567-677`): pass 1 FTS5 BM25, pass 2 sqlite-vec KNN k=100, RRF k=60, **fail-closed partition assertion**, `min_tier` floor + §6.4 tier down-weighting ×1.0/0.8/0.5, source-episode diversity cap 2, token budget. `agent_scope`: `Own`/`OwnAndProject`; `Explicit` is a typed refusal. **No pass-3 KG-BFS** (correct — gated behind a future card).
- Resolver (`src/resolver.rs:15-33`): higher trust rank supersedes; same tier supersedes only if `new.confidence * 1.2 > existing.confidence`; ties keep existing.
- Mediation (`src/mediation.rs:24-85`): `commit_proposal` is the single §6.7 point — screens all text fields, forces `ModelInference`, journals receipt "memory updated for {agent}". Direct `write_*` at ModelInference tier → `MediationRequired` (`store.rs:73-78`).
- Durability: journal-first with unique `memory-N` op ids, writer `FileLock`, network-FS refusal, kill-mid-write fault-injection seam, sibling-build + atomic-replace `rebuild_from_journals` that re-validates partition and skips unreceipted ModelInference rows.
- Tests already green: `tests/retrieval_recall.rs` (recall@10 ≥ 0.90 + zero leak over sealed fixture), `durability.rs`, `write_mediation.rs`, `corrective_regressions.rs` (incl. rebuild query-equivalence at `:249`).
- Fixture exists: `gates/fixtures/memory-retrieval-recall-v1/` (50 facts / 10 decisions / 20 labeled queries / cross-project + two-agent duplicate pairs), validated by `gates/validate-memory-recall-fixture.cjs` + `gates/tests/memory-recall-fixture.test.cjs`.

## 4. Gate machinery — what authoring `mem-sec` involves

- `gates/registry.json` (schema 1) is the closure authority: per-gate `card`, `script`, `closure` (argv/env/cwd_policy/wrapped_tools), `closure_digest` (canonical-JSON SHA-256), `run_artifact`. Validated by `crates/nano-verify/src/registry.rs:92-284`; check-id shape `[A-Z]{2,4}-[0-9]{2}`.
- Card format: YAML frontmatter — `gate_id`, `checks` (id/category/desc/measures), `gate_script_hash`, sealed fixture refs (`sealed:dir-sha256:`), fluent-but-wrong mutant pool (pool_min 5), `gamed_modes`, `escape_hatch_bans`, `last_validated` (see `gates/config-schema/card.md`; parsed by `gates/lib/card.cjs`).
- Output contract: `FAIL <ID> <category>` lines + exactly one `gate: N/M` summary (`crates/nano-verify/src/gate.rs:919`).
- Execution: `wayland-nano verify --gate <id> --run-only` (`crates/nano-cli/src/verify_cmd.rs:8, 331-377, 980-996`).
- CI: `.github/workflows/gate.yml` job `gate-cards` (line 389) runs **Windows-2022 only**, inside an F:-VHDX with restricted-token launcher, and **hardcodes the three-gate inventory in four places**: assert-list (`gate.yml:455-458`), C# allowlist (`:710` + needle `:841`), capability probes (`:775`), run loop (`:972-976`). All four must change for `mem-sec`; §6.5 additionally demands the pack on all 7 CI legs, so a matrix/leg story is part of the work.
- MEM-SEC-3/6's "pass-1, pass-2, fused" checkpoints map exactly onto `store.rs:607/624/650`.

## 5. Admission/resume paths — where the seam plumbs in

- One shared gate: `SharedAdmission::open_production` (`crates/nano-cli/src/activation.rs:21-43`); raw frames admitted before serde (`activation.rs:49-84`; dispatch `acp_mode.rs:5973-5980`).
- `session/new` (`acp_mode.rs:2183-2528`): admission required, `resume_fingerprint` = SHA-256 of receipt, `SessionBegin` journaled, prefix cache built.
- `session/load` (`acp_mode.rs:2530-3060`): authority revalidation (`recheck_session` + `mark_dispatch_eligible`, `:2576-2586`), full journal read fail-closed, transcript replay, resume marker, `ContextFold::prime`.
- Continuity carrier: `session_resume` requires session_id + fingerprint (`crates/nano-activation/src/lib.rs:690-701`); `validate_continuity` fails with `ResumeDrift` (`admission.rs:878-913`).
- CLI exec mints its own carrier: `continuity.strategy = session_id.is_some() ? "session_resume" : "fresh"` (`activation.rs:186-253`).
- **Continuity mode vocabulary is already signed**: `ContinuityStrategy::{Fresh, SessionResume, MemoryRecall}` + `Fallback::{None, Fresh, MemoryRecall}` (`lib.rs:252-274`). `memory_recall` today is a hard `ContinuityNotEnabled` refusal (`admission.rs:884-893`). Phase 3 flips that arm with pinned fallback semantics.
- Four seam points: `acp_mode.rs:3607` (context injection), `acp_mode.rs:4493` (tool surface), `host_mode.rs:291-294` (protocol-host), `exec_run.rs:309-322` (exec — currently nothing).
- Open design point: `MemoryStore::open(nano_home, journal_path, policy)` takes its own journal — session `JournalCoordinator` vs dedicated memory journal must be decided before wiring. Also: only `MemoryPolicy::default()` exists; `validate_policy` is the fail-closed gate (`store.rs:1020-1038`).

## 6. Continuity measurement substrate

- `scripts/soak/` drives the real binary over ACP against the `soak-fake-model` feature seam (deterministic scripted model): `budgets.json`/`budget-eval.mjs` medians-vs-baseline, `verify-journal.mjs`, modes `receipt|ci|smoke`, seeded RNG, NDJSON evidence (`scripts/soak/soak.mjs:1-60`). Natural chassis for the three-mode comparison.
- `wayland-nano session fork <id> [--at-turn]` (`session_cmds.rs:77-84` via `crates/nano-session/src/fork.rs`) is a plausible substrate for benchmark session setup.
- The recall fixture + `retrieval_recall.rs:42-86` remain the retrieval-quality instrument; labels never change in response to retrieval output.

## Risks / watch-items for the planner

1. `kg_nodes`/`kg_edges` have no writers — do not "finish" KG pass-3; it is explicitly anti-scope this phase.
2. `gate.yml`'s hardcoded inventory ×4 makes `mem-sec` a CI-ownership change, not just a `gates/**` change; both ride protected review, and §6.5's 7-leg requirement forces a leg-expansion decision (full matrix vs dedicated legs).
3. The journal-topology decision (session journal vs dedicated memory journal) is the one genuine architecture fork; it gates the seam plan and the rebuild-equivalence proofs.
4. MEM-SEC semantics mostly already have Rust-test coverage — the cards must exercise them through the WP-3 `verify --gate` entry, not restate unit tests.
5. Legacy migration must not let the `.md` store regain authority; quarantine backstops stay until migration evidence lands (AGENTS.md fail-closed rule).
