# Tracked follow-ups

Open items that were consciously deferred from a merged phase. Each entry
records why the deferral was accepted and what closing it means. Owner
promotes/closes entries; builders append only.

## F-1: Global engine-side tool-result size ceiling

- **Filed:** 2026-08-12, as the pre-merge condition of C3+C4 (Q5, both panel
  lenses: per-tool caps suffice for that phase ONLY because every output path
  carries its footer/metadata inside its own cap).
- **Gap:** tool results flow into model history and ACP frames uncapped:
  `crates/nano-agent/src/turn.rs` clones `ToolOutcome.output` verbatim into
  the message history and `crates/nano-protocol/src/acp.rs`
  (`tool_call_done`) carries `rawOutput` uncapped; desktop-core
  `Event::ToolResult.output` likewise. A future tool (or a composed MCP
  tool) that emits an unbounded result bypasses the per-tool caps
  (fs_read 100 KB/page, shell 256 KB, web_fetch 64 KB).
- **Close means:** one engine-side ceiling at the turn.rs history-append
  seam (and the ACP emission seam), fail-closed with a typed truncation
  marker, plus an adversarial test proving an oversized MCP/tool result
  cannot flood the context or corrupt an ACP frame.

## F-2: Desktop-side rendering of ACP diff content blocks

- **Filed:** 2026-08-12, C10 (agent-UX pack), design §6/§10.
- **Gap:** nano emits the ACP-standard `{"type":"diff", path, oldText,
  newText}` content block on `tool_call_update` for fs_write/fs_edit;
  Desktop's ACP adapter preserves `toolCallUpdate.content` in its card
  model (AcpAdapter.ts:245, :267-285) but has NO renderer for ACP diff
  blocks (grep for oldText/newText under desktop `acp/` is clean). The
  nano-TUI is the v1 renderer (changed region with -/+ coloring).
- **Close means:** a Desktop-side renderer for the ACP diff content block,
  driven against a live acp-host write, asserting the card shows the
  before/after. Desktop work, explicitly out of C10's scope.
## F-3: Desktop legacy ACP stack drops error code/data (AcpConnection.ts)

- **Filed:** 2026-08-12 by C7 (ERROR-UX), per the design's Q4 conditional
  resolution (shared/reviews/panel-tui/C7-error-ux-design.md §5.5, §10.4).
- **Finding:** the legacy Desktop stack (`desktop/src/process/agent/acp/
  AcpConnection.ts:647-649`) rejects with `new Error(message)`, dropping
  the JSON-RPC code and `data` — including C7's typed `data.nanoError`.
  Reachability check (2026-08-12, grep-level): the ONLY instantiator of the
  legacy `AcpConnection` is the legacy `AcpAgent`
  (`desktop/src/process/agent/acp/index.ts:203`), and no code path
  instantiates `AcpAgent` — `workerTaskManagerSingleton` builds
  `AcpAgentManager`, whose agent is `AcpAgentV2` (which delegates to the NEW
  `AcpSession`). The legacy stack is dead code for nano (and for every other
  backend reachable today). The `model_not_found` message grep at
  `agent/acp/index.ts:384-387` is likewise unreachable for nano.
- **Close means:** a two-line pass-through (reject with an error preserving
  `code`/`data`) IF a future Desktop change re-arms the legacy stack, plus
  one CDP session proving whether any nano path can reach it. Until then,
  fixing dead code buys nothing.
## F-4: Task-dir GC is manual (C6)

- **Filed:** 2026-08-12, with the C6 background-tasks build (design §10:
  task dirs are RETAINED for debugging; auto-deleting audit artifacts is
  the wrong default).
- **Gap:** `<nano_home>/tasks/<task_id>/` (journal.jsonl, workspace copy,
  report.md) accumulates across sessions. Manual GC: delete
  `<nano_home>/tasks/` (or individual task dirs) when done; nothing
  references them after the owning session ends.
- **Close means:** a `nano tasks gc`-style command with an explicit
  retention policy (age/completion-state), never silent auto-reaping.

## F-5: Desktop adapter lacks C9 surfaces (steer / reconnect / rate-limit / new error kinds)

- **Filed:** 2026-08-12, from the C9 adversarial proof (leg 7): the Desktop
  ACP adapter has no `session/steer` affordance and no rendering for the
  C9 observation notices (reconnect banner, rate-limit snapshot, inert-param
  notice) or the C9/C11 error kinds (model_output_schema,
  model_unsupported_param, session_fork_failed, goal_op_failed).
- **Gap:** TUI carries all of these; Desktop degrades safely (unknown
  update kinds normalize to no-ops — proven by vitest, 10/10), so a
  Desktop user simply never sees steer/reconnect UX and gets generic
  error text for the new kinds.
- **Close means:** Desktop PR adding the steer input path + notice
  renderers + the regenerated error-table mappings (the 40-kind TS module
  already ships on PR #953), plus one CDP drive per surface.

## F-6: Cron job creation has no production path (C11 proof, F-C11-3) — OWNER RULING NEEDED

- **Filed:** 2026-08-12, from the C11 adversarial proof.
- **Gap:** the `cronjob` tool is absent from the interactive ACP tool list,
  and exec mode auto-denies `create` — so no shipped path can create a
  cron job; the proof authored `jobs.json` externally to exercise the
  (fully proven) fire path. Design §5.5's "prompts even under full_auto"
  is unreachable.
- **Close means:** an owner ruling — ship cron creation (add the tool to
  the ACP list with the gate prompt) or formally descope it to
  host-managed-only and amend the design text.

## F-7: Permission-parked turn silences the host ≥15s (C11 proof, F-C11-6)

- **Filed:** 2026-08-12, from the C11 adversarial proof (live probe).
- **Gap:** while a turn is permission-parked, acp-host answers NOTHING —
  fork, second prompt, and even cancel were unobserved for ≥15s
  (single-threaded runtime + synchronous gate wait). The typed
  `turn_in_progress` contract holds only against model-active turns.
  The F-C2-1 relay covers cancel + de-escalation on the READER thread;
  this probe suggests a live-path wedge the wire tests don't capture
  (possibly the probe's park simply outlasted the observation window —
  the evidence section has the repro).
- **Close means:** reproduce under the scripted harness with a parked
  prompt + explicit cancel; if the relay genuinely starves, move the
  gate's wait off the synchronous path (queued approvals) — design
  decision, not a patch.

## F-8: C11 hardening candidates (C11 proof, F-C11-1/2/4/5)

- **Filed:** 2026-08-12, documented-not-patched per proof discipline.
- **Items:** cron idempotency check runs outside the SessionGuard
  (narrow cross-process double-fire window); protocol-host has no cron
  wiring (acp-host only); goal driver swallows journal failures
  (`let _ =`) and degrades context silently on read failure; exec
  mid-turn journal append failure continues with a journal hole.
- **Close means:** one hardening pass against the journal-first
  discipline; each site either journals fail-closed or documents why
  the degradation is the intended semantics.

## F-9: AGENTS.md mid-session edits picked up one turn late in acp_mode (C10 proof, F-C10-1)

- **Filed:** 2026-08-12, from the C10 adversarial proof (pinned by
  `agents_md_edit_between_turns_is_one_turn_late_in_acp_mode`).
- **Gap:** design §4 says a mid-session AGENTS.md edit takes effect
  "next turn"; in acp_mode the prefix rebuild is POST-turn
  (`acp_mode.rs:2256-2271`) while the prompt path clones the cached
  context (`:1423`), so the edit lands one turn later than designed
  (turn N+1 stale, turn N+2 fresh). host_mode re-reads per turn and
  meets the rule (`host_mode.rs:174`).
- **Close means:** rebuild (or invalidate) the context prefix at
  prompt time in acp_mode, or amend design §4 to state the one-turn
  lag explicitly. Prompt-tier data only — no policy impact.

## F-10: TUI question modal clips the 4th option (C10 proof, F-C10-2)

- **Filed:** 2026-08-12, from the C10 adversarial proof (pinned by
  `c10_tui_question_dismiss_viewport_pin`).
- **Gap:** the ask_user modal's scroll window is item-indexed
  (`render.rs:303-307`) while each option renders as TWO rows
  (name + kind); with 4 options (3 minted + Dismiss) the Dismiss row
  is clipped out of the 5-row viewport and never becomes visible,
  even when selected. Still operable blind (Down×3+Enter sends
  `reject`); Esc maps to the reject id. 2-option flows (all standard
  permission cards) unaffected.
- **Close means:** make the viewport row-aware (scroll by rendered
  rows, not item index) so the selected row is always visible.

## F-11: Doc deviation — failed read-before-overwrite emits an add-style diff (C10 proof, F-C10-3)

- **Filed:** 2026-08-12, from the C10 adversarial proof (code review).
- **Gap:** design §6 says a failed read-before-overwrite "omits the
  diff"; the implementation emits an add-style diff (`old_text: None`
  covers both new-file and unreadable-prior, `fs.rs:400-413`) —
  deliberate and code-commented; the add-style diff discloses nothing
  the write itself doesn't.
- **Close means:** amend design §6 wording to match the implemented
  (intended) behavior. Documentation-only.

## F-12: No currentMode push on tool-driven plan transitions (C10 proof, F-C10-4)

- **Filed:** 2026-08-12, from the C10 adversarial proof (demonstrated
  live: Desktop chip held "Plan Mode" after an approved exit).
- **Gap:** tool-driven plan entry/exit (`enter_plan_mode` /
  `exit_plan_mode`) emits NO currentMode notification — v1 has no
  such wire affordance (`session_modes_value` is only written in the
  set_mode handler) — so a client's mode display (TUI status slot,
  Desktop chip) reads stale until its next set_mode. set_mode acks
  themselves are consistent.
- **Close means:** design decision — either add a currentMode push on
  tool-driven posture transitions (wire change, needs Desktop
  coordination) or document that clients must re-query on turn end.

## F-13: Desktop lane — createSession null deref under polluted multi-prompt state (C10 proof observation)

- **Filed:** 2026-08-12, observed once during the C10 Desktop CDP
  drive (`c10-desktop-probe.png`): a second prompt sent while an
  ask_user question card was still open produced
  `Cannot read properties of null (reading 'createSession')` in the
  Desktop task panel. Did NOT recur in any clean single-flow drive.
  Desktop-side robustness, not Nano code — reported for the Desktop
  lane; no Nano action.
- **Close means:** Desktop hardens its session-handle lifecycle
  against a prompt arriving while a question card is open (queue,
  reject with a typed error, or disable the input mid-question).

## F-14: Provider error bodies read unbounded (C7 proof, F-C7-1, severity low)

- **Filed:** 2026-08-12, from the C7 adversarial proof (1 MiB body leg).
- **Gap:** `read_error_body` (`flux_common.rs:40-42`) reads the error
  response body UNBOUNDED (`response.text()`), and `classify_status`
  carries the provider's `error.message` whole into
  `ModelError::Server.message` → logs-side `TypedError.detail`. The
  design's "bounded logs-side" expectation has no size cap on this
  path (the 8 MiB SSE caps bound streaming bodies, not error bodies).
- **Exposure:** transient in-process memory only — the detail never
  reaches the wire, journal, or UI (all statically bounded,
  canary-proven). Flux-only endpoint via egress policy.
- **Close means:** cap the error-body read (e.g. 64 KiB) and truncate
  the carried message with an explicit marker.

## F-15: Desktop lane — C7 typed-error presentation polish (C7 proof leg 8)

- **Filed:** 2026-08-12, all Desktop-side, non-blocking; typed data
  on the wire is proven correct.
- **Items:** (1) failed execute-kind cards don't display the typed
  presentation text ("Denied by user" IS on the wire in `content` +
  `_meta.nanoError`; the card shows only "failed"); (2) typed
  set_model re-apply failures other than `model_not_found` (e.g.
  `provider_key_missing`) surface console-only — `AcpAgentManager`'s
  error emission matches only `model_not_found`; (3) the turn-fatal
  banner folds raw `{"nanoError":…}` JSON into the text and rendered
  twice in the drive; (4) the header pill mislabels typed failures as
  "Connection error"; (5) the compaction-tip renderer threw 4×
  "Error handling notification Object" during the compaction-notice
  turn (cosmetic; notices also journaled).
- **Close means:** Desktop renders the typed presentation on failed
  cards, surfaces ALL typed set_model failures in the UI, strips the
  raw JSON fold from banner text, labels typed failures distinctly
  from transport errors, and fixes the notification renderer's throw.

## F-16: Untyped tool failures render as bare "failed" on cards (C5+C6 proof observation)

- **Filed:** 2026-08-12, from the C5+C6 both-UIs drive.
- **Gap:** untyped tool failures (task fan-out refusal, memory
  refusals) reach clients as failed `tool_call_update` frames whose
  only payload is the `len:N` digest in `rawOutput` — the
  human-readable refusal text goes to the MODEL but not the wire
  frame (frames are built from the digest-only journal ops,
  `acp_mode.rs:3096-3105`). Typed (C7) failures carry the static
  presentation; untyped ones render as a bare "failed".
- **Close means:** either give the task/memory families typed kinds
  (preferred — one table) or surface the refusal text on the card.

## F-17: Wire-contract note — session/cancel with a JSON-RPC id never fires (C6 proof leg 14)

- **Filed:** 2026-08-12, fidelity observation, spec-conformant
  behavior.
- **Note:** ACP defines `session/cancel` as a notification. A cancel
  sent WITH an id (non-spec) is queued as a request and never fires
  the cancel flag. Spec-conformant clients (Desktop sends a
  notification) are unaffected. Also observed: a mid-stream cancel
  answers after the in-flight streaming response completes (flag
  checked at step boundaries; 46.8s worst case in the drive).
- **Close means:** document both behaviors in the ACP extension notes
  for third-party client authors; optionally detect + warn on an
  id-carrying cancel.

## F-18: Provider-side 404 (retired model) surfaces as model_server_4xx, not model_not_found (provider live proofs)

- **Filed:** 2026-08-12, from the live provider-proof matrix (cerebras,
  fireworks: retired model ids returned provider 404s).
- **Gap:** Nano's typed `model_not_found` is only the advertisement-gate
  error (unknown namespaced id). A provider that 404s a retired model
  mid-turn surfaces as `model_server_4xx{status:404}`. Callers keying
  fallback/model-retirement logic off the KIND will miss it.
- **Close means:** either map provider 404-with-model-not-found bodies
  to `model_not_found` at classify time, or document that fallback
  logic must check `status == 404` on `model_server_4xx`.

## F-19: A ':' inside a WAYLAND_NANO_PROVIDERS model entry bricks the whole payload (provider live proofs)

- **Filed:** 2026-08-12, from the live provider-proof matrix
  (OpenRouter's live /models list carries ids like
  `openai/gpt-5-mini:batch` / `:free`).
- **Gap:** the payload parser expects bare model ids; one entry
  containing ':' fails validation and the WHOLE payload is discarded
  fail-closed (Flux-only fallback; with no Flux key the host exits at
  startup). Naive passthrough of OpenRouter's /models list into the
  payload bricks routing.
- **Close means:** host-side normalization — strip `:suffix` tags (or
  reject only the offending entry, keeping the rest of the payload).
  Fail-closed on the whole payload is too blunt for a single bad id.

## F-20: Provider model-id freshness — live lists churn (provider live proofs)

- **Filed:** 2026-08-12. Several 2024/2025-era ids are retired as of
  2026-08 (deepseek-chat, cerebras llama3.1/3.3, fireworks llama-v3p*).
  Nano carries no hardcoded model lists (models arrive via the
  payload), so this is guidance for payload producers (Desktop):
  refresh advertised models from the provider's live /models list
  rather than pinning static lists.
- **Close means:** Desktop payload builder refreshes from live
  /models (with a short cache) or accepts a user-typed model id.

## F-21: Flux grounding is query-phrasing-dependent (P1 adversarial proof) — FIXED at 9849541

- **Filed:** 2026-08-12, from the P1 adversarial live proof (finding F-P1-2, HIGH).
- **Gap:** Flux `{"type":"web_search"}` grounding only fires reliably when
  the query is phrased as a search command (8/8 probes grounded, 20
  results each). Bare keyword queries — what models actually type
  organically — returned an ungrounded prose completion (0/4 probes),
  surfacing as a typed parse failure. Organic use of web_search failed
  without phrasing steering.
- **Fix:** `FluxSearchBackend` shapes the grounding query with a constant
  prefix (`shape_grounding_query`, web_search.rs) — search-command
  phrasings pass through byte-identically. Constant prefix only; the Flux
  isolation property (no conversation context on the wire) is untouched.
- **Verified by:** unit pin + loopback wire pin at 9849541. Live organic
  re-proof (keyword queries must ground with citations, no steering) is
  the closing evidence — to be appended to CONSOLIDATED-VERIFICATION.md
  P1 section when run.

## F-22: ChainedSearchBackend masked real backend failures as "unavailable" (P1 adversarial proof) — FIXED at 9849541

- **Filed:** 2026-08-12, from the P1 adversarial live proof (finding F-P1-1,
  MEDIUM). Originally web_search.rs:609.
- **Gap:** when every CONFIGURED backend failed, the chain returned
  `Unavailable` ("no search backend configured") instead of the real typed
  error — the model saw a misleading "not configured" message and could
  not adapt (e.g. report the actual Flux failure).
- **Fix:** the chain now propagates the LAST configured backend's typed
  `Backend { backend, kind }` error (naming the backend); `Unavailable`
  is reserved for "nothing resolved/configured" (construction-time tail
  or empty ladder). `Cancelled` remains terminal at every layer.
- **Verified by:** updated chain pin
  (`chain_all_down_propagates_the_last_backend_error`) + clippy/workspace
  gates at 9849541.

## F-23: task_spawn failure result lacks a typed error_kind (P1 adversarial proof) — OPEN, structural

- **Filed:** 2026-08-12, from the P1 adversarial live proof (finding F-P1-3,
  LOW). `crates/nano-agent/src/tasks.rs:1352` routes spawn failure through
  `TaskToolExecutor::error` with `error_kind: None`.
- **Why still open:** attaching a kind is NOT a fix-slice change.
  `NanoErrorKind` (`crates/nano-session/src/error_kind.rs`) is the closed,
  serde-pinned journaled op vocabulary with no task-family variant; the
  spawn-failure `TaskError`s (`FanOutCap`, `DepthLimit`, `WorkspaceCopy`,
  `DriverUnavailable`) map to nothing existing without misusing kinds
  (e.g. `FsIo` for `WorkspaceCopy` would be dishonest). Doing this right
  needs a new variant added to the wire vocabulary.
- **Close means:** a design/panel decision adds a task-family variant to
  `NanoErrorKind` (serde-compatible vocabulary extension) and threads it
  through the spawn-failure path.

## F-24: Backend name does not cross the ACP wire card or journal-rebuilt context (P1 re-proof, leg R2 residual) — OPEN, small

- **Filed:** 2026-08-12, from the P1 fix re-proof (leg R2). The masking bug
  itself is dead (9849541): the failing turn's model-facing error names the
  backend (`web_search backend brave failed: ...`) and the wire card carries
  the real typed kind (`model_server_4xx`), never the "unavailable" mask.
- **Residual:** the backend-naming string does not survive (a) the ACP wire
  card — digest + static presentation by C7 design — or (b) the
  journal-rebuilt cross-turn context (`<presentation> [output elided]`). A
  verbatim-quote follow-up reproduces the static presentation, not the
  backend-naming string, so a later turn cannot learn WHICH backend failed.
- **Close means:** include the backend name in the C7 wire-card presentation
  payload (or the elided journal rebuild) for backend-typed tool errors.

## F-25: Blob-store GC cross-process battery is single-process theater (P2a code audit, codex MEDIUM-3) — OPEN

- **Filed:** 2026-08-13, from the P2a merged-code panel audit (codex lens,
  finding MEDIUM-3). The certified §12 cross-process GC race battery is not
  present: `sweep_skips_under_a_writer_lease`
  (crates/nano-session/src/attachment_store.rs:1109) uses the same store
  handle in one process and merely asserts that this represents process B.
- **Close means:** a real helper-process test where process A holds the
  shared lease across publish/journal pause and process B attempts the
  exclusive sweep; repeat without the lease to exercise the grace guard.

## F-26: `WriteLease` is not bound to store identity (P2a code audit, codex MEDIUM-4) — OPEN

- **Filed:** 2026-08-13, from the P2a merged-code panel audit (codex lens,
  finding MEDIUM-4). `put` (crates/nano-session/src/attachment_store.rs:237)
  accepts any `WriteLease` without proving it protects this store; a lease
  acquired from store A can authorize publication into store B while B's GC
  runs.
- **Close means:** bind `WriteLease` to the canonical store/lock identity
  and reject mismatches in `put`, with a two-store regression test.

## F-27: P3 dispatcher merge-review LOW debt (8 items, reviewer agent-131, 2026-08-13) — OPEN

- **Filed:** 2026-08-13, from the independent pre-merge review of
  `feat/p3-dispatcher` (verdict MERGE-OK; gates green; merged at 8b2412a,
  integrator repair 8d8425d).
- **Items:**
  1. §2.2 undocumented deviation: `enqueue_priority_handler`
     (`nano-mcp/src/dispatcher.rs:650-665`) grants a 500ms bounded
     drain-retry before poisoning (note names no such retry; rationale is
     §12(h)'s 17-through-16 requirement). Record-only or note amendment.
  2. §2.3 letter-deviation: child exit reaped by supervisor-tick
     `try_wait` polling (`dispatcher.rs:1258-1269`), not a wait thread.
  3. Graceful-close poison-reason race: writer drain-exit always emits
     `SupEvent::WriterDone` (:1177) which the supervisor maps to poison
     "writer queue disconnected"; if it lands before `Shutdown` a clean
     close records an alarming reason. Fix: suppress WriterDone when
     `shared.closing()`.
  4. §2.6 per-pending-id `notifications/cancelled` can silently miss the
     wire: `Connection::shutdown` (:1460-1478) enqueues cancels after
     `set_closing`; the writer's closing drain may exit first and the Err
     is discarded unlogged. Fix: enqueue cancels before set_closing, or
     log the drop.
  5. Dead test-only surface: `StdioTransport::from_pipes`
     (`stdio.rs:51-53`) is cfg(test) with zero callers. Delete.
  6. §2.1.5/§2.4 unwired one layer up (declared deferral):
     `nano-agent/src/mcp.rs:138` holds the registry mutex across blocking
     `call_tool_mutable`; `execute_cancellable` still delegates to plain
     `execute` despite `call_tool_cancellable` existing
     (`client.rs:281-288`). Fix belongs to a nano-agent lane:
     lock-to-clone + call the cancellable path; until then turn-cancel of
     an in-flight MCP call is not end-to-end.
  7. §2.6 contained spawn not in this branch (scope split, honestly
     documented): supervisor kills via bare `child.kill()`;
     `spawn_process_with_pipes_contained` lands with the oauth-egress
     lane; stdio-MCP capability flag pinned FALSE until §13 leg-1b.
  8. Test-coverage gaps vs §12 (non-blocking): no writer-thread
     fault-injection leg (reader has one); §12(j) sustained-priority-flood
     asserted at scheduler unit level, not on the wire; §12(k)
     process-inventory oracle deferred to §13 leg-1b.
- **Close means:** each item fixed or explicitly waived with the reason
  recorded here.

## F-28: P4 repomap merge-review debt (reviewer agent-135, verdict MERGE-OK, 2026-08-13) — OPEN

- **MEDIUM-1 (cost-bound TOCTOU) — FIXED at 02b538a:**
  `nano-repomap/src/store.rs:321-345`
  enforces the 5 MB per-file cap via pre-read metadata only; the streaming
  `read_line` loop is unbounded, so a growing or long-line file blows the
  §5.2 cost bound. Fix: `Read::take(max_file_bytes + 1)` + record Oversize.
- **MEDIUM-2 (integration obligation, NOT optional):** `repo_map` ships
  unwired by lane split (`nano-tools/src/repomap.rs:9-21`). The wiring
  lane MUST land §5.5's read-only registration at all three gates
  (acp_mode.rs, tasks.rs, exec_mode.rs) WITH the three-predicate agreement
  regression test in the same merge that registers the tool.
- **MEDIUM-3 (perf) — FIXED at 02b538a:** every refresh pass re-walks the whole tree and
  re-reads hashed files under the tool Mutex with no file-count cap or
  per-pass timeout; one pass on a huge repo stalls all queries. Fix:
  document the total-pass bound or add a cap + `skipped_over_cap` counter.
- **LOW-4 — FIXED at 02b538a:** `find_rename_source` (store.rs:311) treats any stat failure
  as disappearance; require `NotFound` via `symlink_metadata`.
- **LOW-5 — FIXED at 02b538a:** `skipped_denied` (store.rs:208) accumulates per-pass counts
  forever; report last-pass count or rename to `_total`.
- **LOW-6 — FIXED at 02b538a:** `query.rs:72` path-token match includes the workspace-root
  prefix; match root-relative instead.
- **LOW-7:** Windows battery owes a unicode-name case and (where the
  volume permits) a >MAX_PATH case; the real-D:\ harness remains a §14
  leg-4 proof obligation outside the branch.
- **LOW-8 (pre-existing, relocated by this branch):**
  `nano-core/src/sensitive_path.rs:25` `.env.` prefix check is
  case-sensitive; `.ENV.PRODUCTION` slips on case-insensitive filesystems.

## F-29: P4 rules merge-review LOW debt (reviewer agent-133, 2026-08-13) — OPEN

- LOW-3: parse-disagreement check (execrules.rs:506-510) inspects only the
  span's leading char; sound today only via positive-grammar side effect.
  Fix: tokenize the raw span under both grammars and compare boundaries,
  or delete the vestigial check + document the argument.
- LOW-4: differential suite sorts segment lists (order-blind) and corpora
  are thin (no cmd position-0 `=` leg, no mixed &&/||, no sh
  backslash-path). Fix: assert order for deterministic connectors;
  multiset only for &/| cases; extend corpora.
- LOW-6: `nano-session/src/lock.rs` mechanism re-implemented in
  nano-core/execrules.rs:1013-1090 (layering forced it; recorded in
  UPSTREAM.md). Optional: extract the lock primitive to a shared leaf.

## F-30: P3 oauth merge-review LOW debt (reviewer agent-132, 2026-08-13) — OPEN

- LOW-5: duplicate Host headers pass if any one matches
  (oauth/loopback.rs:199-202, `.any()`); RFC 7230 wants 400. Cheap fix.
- LOW-6: wincred `write()` leaves a torn chunk prefix on mid-chunk
  failure (reads still fail closed via serde parse). Optional: clean up
  chunks 0..n on failure.

## F-31: P4 PTY merge-review LOW debt (reviewer agent-134, 2026-08-13) — OPEN

- LOW-5: default `after_offset` cursor is `state.output.oldest`
  (pty.rs:380), not the note-letter per-reader resume; document the
  default in the tool schema or track a per-session last-served offset.
- LOW-7: no ConPTY-path CREATE_BREAKAWAY_FROM_JOB rejection test (property
  rests transitively on job.rs's spawn_contained test); broker-escape
  record is a string-constant self-test; §14 leg 3(c) owes the
  schtasks/WMI survivor count at integration.
- LOW-8: `unix_sandbox_argv` defined only for linux/macos — other
  cfg(unix) targets fail to compile (fail-closed direction, at least);
  session-cap test is Windows-only.

## F-32: P2b image-results merge-review LOW debt (reviewer agent-137, 2026-08-13) — OPEN

- LOW-5: unavailable-label text diverges from the design's fixed string —
  `acp_mode.rs:4182-4190` + `image_result.rs:86` render dims instead of
  "[Image #N from tool <unavailable: unpaired call> — do not describe it
  from memory]". Fix: emit the fixed label in the None branch.
- LOW-6: non-view_image image-bearing Live results are silently dropped
  (turn.rs:1347 gates typed rejection on call.name == "view_image").
  Fix: reject whenever image_result.is_some() && accepted.is_none().
- LOW-7: AttachmentStore::sweep's reference-scan contract names only
  input_blocks manifests (attachment_store.rs:367-369); §3.2 requires
  covering ToolResult.image_refs when the host scan is wired. Latent —
  no production caller exists yet; record + extend at wiring.
- LOW-8 (info): ReplayVerified minted-then-dropped at replay
  (acp_mode.rs:4222-4236) rather than "carried" per §3.3 (harmless —
  Message cannot hold it); build_image_tool_result's _call_id unused
  (binding is via Op.call_id) — cosmetic.

## F-33: P2a proof process follow-ups (proof agent-128, 2026-08-13) — OPEN

- **Polyglot semantics conflict:** §13 leg 3 says polyglot PNG+ZIP is
  "typed-rejected"; the certified §4.1 trailing-payload tolerance +
  shipped code ACCEPT then gate-reject, and the re-encode is digest-
  identical to the payload-free control (the payload never influences
  output). One should be amended — recommendation: amend §13's wording to
  "accepted-then-stripped, proven byte-inert", since the §4.1 tolerance is
  itself certified and the strip is byte-proven.
- **Wire naming note:** the note says `_meta.nanoError.kind`; the shipped
  wire carries `error.data.nanoError.kind`. Reconcile the note's spelling
  (code is the contract per C7).
- **magistral-medium re-probe:** INCONCLUSIVE at leg 6 (Flux 400 "no
  healthy deployments" on all calls incl. text-only control). Single
  re-probe if Flux restores the deployment; catalog stays false meanwhile.

## F-34: AttachmentStore GC sweep has no production caller (P2a proof, leg 4 GC leg) — OPEN, production finding

- **Filed:** 2026-08-13, from the P2a adversarial proof (agent-128):
  `AttachmentStore::sweep()` is implemented and unit-tested but NEVER
  invoked — `/doctor` neither sweeps nor reports the store, and acp-host
  startup doesn't sweep either (planted aged unreferenced blob + stale
  `.tmp` survived both: `.tmp/p2a-proof/captures/leg4cd.json`,
  `leg4d2.json`). Design §5.4 specifies "sweep invocation: host startup +
  /doctor". Unreferenced blobs currently accumulate forever.
- **Close means:** wire the sweep at host startup (lease+grace discipline
  already in-crate) and a `/doctor` store report (size, blob count);
  re-run the leg-4 GC concurrent leg (sweep racing a live attach — blob
  survives) as the promotion proof. NOTE: F-32 LOW-7 (sweep's reference
  scan must cover ToolResult.image_refs) must land first or with it.
