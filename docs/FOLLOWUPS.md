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

## F-6: Cron job creation has no production path (C11 proof, F-C11-3) — FIXED (S6)

- **Filed:** 2026-08-12, from the C11 adversarial proof.
- **Gap:** the `cronjob` tool is absent from the interactive ACP tool list,
  and exec mode auto-denies `create` — so no shipped path can create a
  cron job; the proof authored `jobs.json` externally to exercise the
  (fully proven) fire path. Design §5.5's "prompts even under full_auto"
  is unreachable.
- **Close means:** an owner ruling — ship cron creation (add the tool to
  the ACP list with the gate prompt) or formally descope it to
  host-managed-only and amend the design text.
- **Resolution (2026-08-14, owner ruling = SHIP with the locked §5.5
  posture, branch `feat/s6-cron-path`):**
  - The `cronjob` tool is registered on the interactive ACP session surface
    (`acp_mode.rs` per-turn build) and serviced journal-first by
    `CronjobExecutor`, now holding the session's `JournalCoordinator`:
    create/delete append `Op::CronCreated`/`Op::CronDeleted` (additive,
    `Unknown`-tolerant) BEFORE the `jobs.json` cache persist — the journal
    is authoritative for job EXISTENCE, exactly as it already was for
    fires.
  - Gate (AcpApproval arm 1f): create ALWAYS prompts the host, in every
    mode including full_auto (the locked C11 ruling — scheduled code
    execution is never auto-approved); delete prompts too; read_only and
    the plan posture typed-deny create/delete; list approves in
    read_only/full_auto, prompts in default. Pinned by
    `c11_cronjob_gate_matrix`.
  - Exec stays typed-denied for ALL cronjob actions in every mode (pinned
    arm in `exec_gate_decision`, like the PTY names): create/delete would
    prompt and exec can never prompt; headless list is deliberately out of
    v1 scope.
  - Kill windows closed by reconciliation: a torn create (CronCreated
    durable, cache persist lost) is rebuilt by the runner's existence
    discovery and fires; a torn delete (CronDeleted durable, cache still
    carrying the job) is removed WITHOUT firing. Tests:
    `torn_create_is_rebuilt_from_journal_and_fires`,
    `torn_delete_removes_cache_entry_without_firing`,
    `created_job_survives_compaction_and_fires`,
    `cronjob_create_journal_failure_leaves_cache_untouched` (nano-agent);
    fold/suppression/round-trip in nano-session fork_tests.
  - Live proof: `scripts/s6-proof/f6_cron_create_proof.py` — the model
    creates the job THROUGH the tool (gate prompt answered over
    `session/request_permission`), the host is killed before the first
    fire, a fresh acp-host resumes and fires it exactly once with the
    provenance-marked input. No externally-authored `jobs.json` anywhere in
    the proof path.

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

## F-8: C11 hardening candidates (C11 proof, F-C11-1/2/4/5) — DATA-INTEGRITY HALF FIXED (W2 cron lane, commit on branch fix/w2-cron); scope half (F-C11-2 protocol-host cron wiring) still OPEN

- **Filed:** 2026-08-12, documented-not-patched per proof discipline.
- **Items:** cron idempotency check runs outside the SessionGuard
  (narrow cross-process double-fire window); protocol-host has no cron
  wiring (acp-host only); goal driver swallows journal failures
  (`let _ =`) and degrades context silently on read failure; exec
  mid-turn journal append failure continues with a journal hole.
- **Close means:** one hardening pass against the journal-first
  discipline; each site either journals fail-closed or documents why
  the degradation is the intended semantics.
- **Split (SEVERITY-SIGNOFF-2026-08-14):** the sev-2 data-integrity
  half (cross-process cron double-fire window + continued execution
  after a failed journal append) is FIXED on branch `fix/w2-cron`:
  - Double-fire window: `tick_one` now re-folds the session journal
    UNDER the session guard (the S3 in-process mutex + OS file lock, or
    the lifetime ownership lock it stands in for) and re-checks both the
    tombstone set and the occurrence reservation before journaling
    `CronFired` — check-and-reserve is atomic across processes. Proven
    by `cross_process_same_occurrence_fires_exactly_once`: two real
    child processes sharing one NANO_HOME tick the same due occurrence
    concurrently; exactly one fire lands in the shared fired-log and
    exactly one `CronFired` reservation in the journal.
  - That proof also exposed a second cross-process defect: the store's
    fixed tmp name (`jobs.jsonl.tmp`) let a concurrent host's save
    clobber the winner's post-reservation persist (durable reservation,
    aborted fire — a lost occurrence). Fixed: per-process tmp name in
    `JsonCronStore::save`.
  - Failed journal append at fire time: verified the fire path already
    aborts BEFORE the cache is touched (`cannot journal CronFired`
    typed `JobTickOutcome::Error`, no injection); pinned by
    `fire_time_journal_failure_aborts_no_execution_no_cache_mutation`
    (torn-path injection at fire time ⇒ no execution, typed error,
    `last_fired`/`next_fire` untouched).
  - NOT in this half (unchanged): protocol-host cron wiring (F-C11-2,
    scope question), goal-driver `let _ =` journal swallow, exec
    mid-turn append failure — those remain open under the original
    items above.

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

## F-10: TUI question modal clips the 4th option (C10 proof, F-C10-2) — FIXED at 405f0e2

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
- **Fix (405f0e2):** the modal height now counts rendered rows (one per
  item plus one per description) and the scroll window is row-aware, so
  the selected item's rows are always in the viewport
  (`crates/nano-tui/src/render.rs`). Verified by two TestBackend unit
  tests (`modal_viewport_keeps_selected_two_row_option_visible`,
  `modal_height_counts_rendered_rows`), the flipped bug-pin
  `c10_tui_question_dismiss_viewport_pin`, and the regenerated
  approval-modal snapshot (the old one had captured the clipped render —
  every two-row approval option's kind line was being clipped too).

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

## F-14: Provider error bodies read unbounded (C7 proof, F-C7-1, severity low) — FIXED (wave-2, sev-2 per 2026-08-14 adjudication)

- **Fixed:** `read_error_body` (`flux_common.rs`) reads bounded at 64 KiB
  via chunked copy; `error_body_read_is_bounded` pins the cap and the
  small-body path. Truncated JSON falls through to generic status arms.

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

## F-17: Wire-contract note — session/cancel with a JSON-RPC id never fires (C6 proof leg 14) — cancel-latency half FIXED at 56d23a8

- **Filed:** 2026-08-12, fidelity observation, spec-conformant
  behavior.
- **Note:** ACP defines `session/cancel` as a notification. A cancel
  sent WITH an id (non-spec) is queued as a request and never fires
  the cancel flag. Spec-conformant clients (Desktop sends a
  notification) are unaffected. Also observed: a mid-stream cancel
  answers after the in-flight streaming response completes (flag
  checked at step boundaries; 46.8s worst case in the drive).
- **Cancel-latency half FIXED (2026-08-14, wave-2 fix lane, sev-2 per
  the 2026-08-14 severity adjudication):** the turn loop now races
  every in-flight `complete_observed` against the cancel flag
  (`cancel_raced`, turn.rs — 25ms watcher, `tokio::select!`); a fired
  flag drops the in-flight future (cancelling the HTTP request) and
  the new `Err(ModelError::Cancelled)` arm maps it to the SAME
  terminal semantics as a boundary cancel — Stopped(user_cancelled) +
  journaled TurnEnd(cancelled) → stopReason "cancelled" — with the
  in-flight reservation settled conservatively first (P1 §3.5). A
  driver-reported boundary cancel now also journals TurnEnd(cancelled)
  (previously it took the generic failure arm with no TurnEnd).
- **Verified by:** engine pin
  `cancel_mid_call_aborts_inflight_response_promptly` (parked driver,
  never released, sub-second abort, TurnEnd(cancelled) journaled) and
  ACP proof `cancel_mid_stream_aborts_inflight_response_and_session_survives`
  (cancel mid-stream answers stopReason "cancelled" in <1s with the
  model never released; the session serves the next prompt).
  Cancel-at-boundary regression
  (`cancel_mid_turn_answers_cancelled_and_stops_stream`) unchanged and
  green.
- **Still open (the documentation half):** document both wire-contract
  behaviors in the ACP extension notes for third-party client authors;
  optionally detect + warn on an id-carrying cancel.

## F-18: Provider-side 404 (retired model) surfaces as model_server_4xx, not model_not_found (provider live proofs) — FIXED at e6a6dca

- **Filed:** 2026-08-12, from the live provider-proof matrix (cerebras,
  fireworks: retired model ids returned provider 404s).
- **Gap:** Nano's typed `model_not_found` is only the advertisement-gate
  error (unknown namespaced id). A provider that 404s a retired model
  mid-turn surfaces as `model_server_4xx{status:404}`. Callers keying
  fallback/model-retirement logic off the KIND will miss it.
- **Fix (2026-08-14, wave-2 fix lane):** `flux_common::classify_status`
  folds HTTP 404 into the new typed `ModelError::ModelNotFound`
  variant. Consumers wired end to end: `kind_of_model` →
  `NanoErrorKind::ModelNotFound` (wire kind `model_not_found`, 404 as
  the closed status extra); the P5 ladder signal fold → terminal
  `RoutingFailureClass::ModelNotFound` with the status journaled;
  `model_error_of_failure_class` reconstructs the typed variant. The
  class stays TERMINAL per the P5 §4 design (a stale advertisement
  fails closed, never cascades) — the fix is the kind mapping, not a
  cascade-semantics change.
- **Verified by:** `flux_common::tests::provider_404_classifies_as_model_not_found`,
  `auto_routing::tests::provider_404_folds_to_typed_model_not_found_end_to_end`,
  the error_map typed-extras pin, and the ladder-level
  `provider_404_journals_model_not_found_and_closes_terminal`
  (journaled failure kind model_not_found, zero calls to the next
  candidate).

## F-19: A ':' inside a WAYLAND_NANO_PROVIDERS model entry bricks the whole payload (provider live proofs) — FIXED at 05103a2

- **Filed:** 2026-08-12, from the live provider-proof matrix
  (OpenRouter's live /models list carries ids like
  `openai/gpt-5-mini:batch` / `:free`).
- **Gap:** the payload parser expects bare model ids; one entry
  containing ':' fails validation and the WHOLE payload is discarded
  fail-closed (Flux-only fallback; with no Flux key the host exits at
  startup). Naive passthrough of OpenRouter's /models list into the
  payload bricks routing.
- **Fix (2026-08-14, wave-2 fix lane):** per-entry malformation
  (empty, namespaced, overlong, or non-string model id) now drops only
  the offending entry with a loud typed `payload_entry_invalid`
  warning, collected on the router (`payload_warnings()`) and surfaced
  on startup stderr (from_env's diagnostic channel) and as a doctor
  `provider-payload` WARN line. Structural malformation (bad JSON,
  wrong entry shapes, oversize bytes/entries/models-per-provider)
  stays wholesale-fatal (`payload_invalid`, Flux-only fallback).
- **Verified by:** `f19_one_bad_model_entry_drops_the_rest_survives`
  (mixed payload keeps its good entries with three typed warnings; a
  structurally malformed payload stays fatal) and the updated
  `payload_validation_battery` (overlong id: drop-with-warning, no
  longer wholesale rejection).

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

## F-28: P4 repomap merge-review debt (reviewer agent-135, verdict MERGE-OK, 2026-08-13) — MEDIUM-2 CLOSED at a7065e0 (three-gate registration + agreement tests landed in the wiring pass); MEDIUM-1/3 + LOWs FIXED at 02b538a; LOW-8 FIXED at 405f0e2; LOW-7 remains OPEN

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
- **LOW-8 (pre-existing, relocated by this branch) — FIXED at 405f0e2:**
  the `.env.` prefix check is now case-insensitive, matching the basename
  equality arms, with adversarial tests for `.ENV.PRODUCTION` /
  `.Env.Production` / `.env.LOCAL`; tightening only (near-misses like
  `.envrc` / `env.production` stay clear). Original text:
  `nano-core/src/sensitive_path.rs:25` `.env.` prefix check was
  case-sensitive; `.ENV.PRODUCTION` slipped on case-insensitive
  filesystems.

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
- LOW-7: FIXED at a7065e0 — referenced_blob_digests(sessions_dir) scans both input_blocks manifests and ToolResult.image_refs, fail-closed on unreadable journals.
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

## F-34: AttachmentStore GC sweep has no production caller (P2a proof, leg 4 GC leg) — FIXED at a7065e0 (startup sweep + /doctor report + image_refs scan; the leg-4 concurrent re-proof is owed to the P2a re-verification pass)

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

## F-35: P3 ToolSearch merge-review LOW debt (reviewer agent-144, 2026-08-13) — OPEN

- **LOW-5 (fidelity):** `McpElicitationUnsupported` typed kind is never
  emitted for the pre-2025-06-18 case — the version gate is behaviorally
  closed (no slot designated, spec-legal -32601) but the §7-named kind
  never surfaces; the config-time HTTP refusal is eprintln-only, never a
  client-visible typed error. Fail-closed today; fidelity gap only.
- **LOW-7 (recorded deviation, verified safe by reviewer):** both
  compaction paths journal `covers_op_ids` = the FULL snapshot
  (acp_mode.rs:2435-2438, :2707-2711), discarding the builder-computed
  per-turn ids (turn.rs:674/975) — a semantic change to a C1 contract.
  Reviewer verified: covers is audit-only at rebuild, resume folds the
  full stream, carry exact by construction; also fixes a latent base bug.
  Recorded here and required in the fix-round commit message.
- **LOW-10:** interrupted-call attribution (mcp.rs:1007-1009) is
  last-writer-wins under concurrency — a journaled call_id can be
  wrong/empty for non-designated concurrent calls. Audit-only field;
  designated-slot binding is dispatcher-correct. Optional: key per-call.

## F-36: OAuth grant op has NO producer on either merged side (reviewer agent-144 checklist item 7) — FIXED at a7065e0 (oauth_grant_recorder at the acp_mode session layer, journal-first, idempotent)

- Master's OAuth lane (`nano-mcp/src/oauth/flow.rs:41,81`) defines
  `record_grant: &dyn Fn(&GrantRecord)` expecting the ToolSearch branch's
  `Op::McpOauthGrant` — but no code on either side implements the hook.
  Until wired, OAuth logins journal nothing (grants exist only in
  memory). Integrator: implement the hook at the acp_mode session layer —
  checked conversion `flow::GrantEndpoint{HttpMethod}` →
  `op::GrantEndpoint{GrantMethod}` (reject Unknown), run
  `validate_oauth_grant`, append through the session coordinator.
  Lands in the RC2 wiring pass (docs/RC2-WIRING-PASS.md §7).

## F-37: P4 session-browser merge-review debt (reviewer agent-146, verdict MERGE-OK, 2026-08-13) — OPEN

- **MEDIUM-1:** FIXED at a7065e0 (`/resume <id>` sends session/load for any syntax-safe explicit id; refuses only present-and-live rows). Original text: `/resume <id>` (nano-tui/src/app.rs:760-773) was gated on
  membership in the BOUNDED 200-entry list — an explicit-id resume of a
  session older than the truncation window is refused with no workaround,
  deviating from §6.2's "`/resume <id>` sends session/load". Fix (lands in
  the wiring pass): explicit-id resume sends session/load directly after
  the client-side syntax check (host re-validates); keep the Live-refusal
  only when the row is present AND live.
- **LOW-2:** `open_regular_no_follow` (session_browser.rs:231) propagates
  ELOOP/NotFound/PermissionDenied from the open via `?`, failing the entire
  listing on a transient swap. Fix: map those to Ok(None) (skip entry).
- **LOW-3:** `let entry = entry?` (session_browser.rs:75) aborts the whole
  list on one enumeration error; the cited precedent flattens
  (bootstrap.rs:241). Fix: flatten.
- **LOW-4:** `print_sessions` error line goes to stdout, not stderr
  (exit code 2 is correct). Cosmetic.

---

## F-38: bwrap probe test is environment-sensitive on CI (ubuntu-24.04-arm) — OPEN, test-robustness

- **Filed:** 2026-08-13, integrator observation. Master commit 061ff9b
  (docs-only) failed CI run 31704608343 on `ubuntu-24.04-arm`:
  `linux_bwrap::tests::linux_probe::system_bwrap_warning_reports_user_namespace_failures`
  panicked with `loopback: Failed RTM_NEWADDR` — the probe executes the
  system bubblewrap and asserts a user-namespace warning is surfaced, but
  the runner environment failed earlier at netlink loopback address
  config, so no warning materialized (left=None). The SAME code passed at
  a7065e0 and the rerun of 061ff9b came back 6/6 green with zero code
  changes — confirmed environmental flake, not a regression.
- **Close means:** the probe test tolerates (or explicitly detects and
  skips/reports) runner environments where netlink RTM_NEWADDR is
  restricted, WITHOUT weakening the user-namespace warning assertion on
  environments that support it. Do not fix by loosening the assertion —
  detect the environment class and branch.
- **Severity:** LOW (flake cost: one rerun cycle; no user-facing impact).

# Pending FOLLOWUPS entries — to ride the next merge commit

## F-39: doctor is report-only for attachment GC; design §5.4 says "startup + explicit /doctor" — LOW, reconciliation
- From agent-159 P2a GC re-proof (all 5 legs PASS, F-34 stays closed). Shipped: sweep at startup only; /doctor reports but does not sweep. Matches F-34's close contract; deviates from P2a design note §5.4's literal text. Fix = either wire a doctor-triggered sweep or amend §5.4 wording. Evidence: .tmp/p2a-reproof/captures/reproof-leg2.json.

## F-40: /doctor sessions-dir-permissions WARN leaks attachment-audit wording — LOW, cosmetic
- From agent-159. On a sessions-less home, /doctor prints a WARN whose detail contains `GetNamedSecurityInfoW failed` (attachment-audit phrasing); rc stays 0. Cosmetic string hygiene.

## P3 proof findings (agent-154, manifest section at shared/reviews/RC/evidence/CONSOLIDATED-VERIFICATION.md:992)
- CRITICAL/HIGH going to fix round (feat/p3-fixround): F-P3-1 (OAuth flow dead code — CRITICAL), F-P3-2 (StdioTransport uncontained children — HIGH), F-P3-3 (registration receipt/instance_id/SpecSource absent — HIGH), F-P3-4 (elicitation op-id reset on resume, answer never journaled — HIGH, live-proven), F-P3-7 (user-cancel maps to decline not cancel — acp_mode.rs:3753-3771), F-P3-13 (backpressure battery host-dependent failure — triage).
- MEDIUM/LOW filed by reference: F-P3-5..F-P3-8, F-P3-10 + LOW list live in the manifest section. S5 resolved F-P3-9 (mismatch-path churn accounting), F-P3-11 (binding-drop listener teardown), and F-P3-12 (bounded HTTP transport/status errors). Unreproduced anomaly: one leg5-timeout rc=1 exit, 2 clean reruns.
- **S5 HTTP follow-up:** the dispatcher bridge currently receives POST responses only. Streamable-HTTP GET/SSE server-initiated requests remain deliberately unadvertised and require a separately egress-gated receive pump plus loopback proof before promotion.

## P2b proof findings (agent-160, manifest section at CONSOLIDATED-VERIFICATION.md:1043-1065)
- **F-P2B-1 HIGH (live-proven both wires):** view_image unreachable in every shipped config — vision_backed conjuncts mutually unsatisfiable (flux leaves bind openai-completions; vision catalog keys are all flux-pinned-*). Unblock options: bless an anthropic:<model> catalog id after live probe, OR wire flux→anthropic compat routing (FluxDriver::anthropic_compat exists, uncalled). DEFER to post-P5-merge wave — the fix surface (provider_router/vision gating) is inside agent-149's active lane.
- **F-P2B-2 LOW:** ToolResult replay degradation arm emits no operator stderr log (comment claims it at acp_mode.rs:4792-4794; notice cause split correct and test-pinned).
- **F-P2B-3 LOW:** rung-3 vision gate is once-per-turn — mid-turn compaction eviction doesn't re-arm until next prompt (fail-closed, self-heals).
- F-32 LOW-6 independently re-confirmed accurate post-wiring.
- Alt B decision RECORDED in the manifest (leg 7). Legs 3/4/5/6 PASS on all expressible paths; leg 2 inexpressible under F-P2B-1.

---

# Pending FOLLOWUPS entries — to ride the next merge commit

## F-39: doctor is report-only for attachment GC; design §5.4 says "startup + explicit /doctor" — LOW, reconciliation
- From agent-159 P2a GC re-proof (all 5 legs PASS, F-34 stays closed). Shipped: sweep at startup only; /doctor reports but does not sweep. Matches F-34's close contract; deviates from P2a design note §5.4's literal text. Fix = either wire a doctor-triggered sweep or amend §5.4 wording. Evidence: .tmp/p2a-reproof/captures/reproof-leg2.json.

## F-40: /doctor sessions-dir-permissions WARN leaks attachment-audit wording — LOW, cosmetic
- From agent-159. On a sessions-less home, /doctor prints a WARN whose detail contains `GetNamedSecurityInfoW failed` (attachment-audit phrasing); rc stays 0. Cosmetic string hygiene.

## P3 proof findings (agent-154, manifest section at shared/reviews/RC/evidence/CONSOLIDATED-VERIFICATION.md:992)
- CRITICAL/HIGH going to fix round (feat/p3-fixround): F-P3-1 (OAuth flow dead code — CRITICAL), F-P3-2 (StdioTransport uncontained children — HIGH), F-P3-3 (registration receipt/instance_id/SpecSource absent — HIGH), F-P3-4 (elicitation op-id reset on resume, answer never journaled — HIGH, live-proven), F-P3-7 (user-cancel maps to decline not cancel — acp_mode.rs:3753-3771), F-P3-13 (backpressure battery host-dependent failure — triage).
- MEDIUM/LOW filed by reference: F-P3-5..F-P3-8, F-P3-10 + LOW list live in the manifest section. S5 resolved F-P3-9 (mismatch-path churn accounting), F-P3-11 (binding-drop listener teardown), and F-P3-12 (bounded HTTP transport/status errors). Unreproduced anomaly: one leg5-timeout rc=1 exit, 2 clean reruns.
- **S5 HTTP follow-up:** the dispatcher bridge currently receives POST responses only. Streamable-HTTP GET/SSE server-initiated requests remain deliberately unadvertised and require a separately egress-gated receive pump plus loopback proof before promotion.

## P2b proof findings (agent-160, manifest section at CONSOLIDATED-VERIFICATION.md:1043-1065)
- **F-P2B-1 HIGH (live-proven both wires):** view_image unreachable in every shipped config — vision_backed conjuncts mutually unsatisfiable (flux leaves bind openai-completions; vision catalog keys are all flux-pinned-*). Unblock options: bless an anthropic:<model> catalog id after live probe, OR wire flux→anthropic compat routing (FluxDriver::anthropic_compat exists, uncalled). DEFER to post-P5-merge wave — the fix surface (provider_router/vision gating) is inside agent-149's active lane.
- **F-P2B-2 LOW:** ToolResult replay degradation arm emits no operator stderr log (comment claims it at acp_mode.rs:4792-4794; notice cause split correct and test-pinned).
- **F-P2B-3 LOW:** rung-3 vision gate is once-per-turn — mid-turn compaction eviction doesn't re-arm until next prompt (fail-closed, self-heals).
- F-32 LOW-6 independently re-confirmed accurate post-wiring.
- Alt B decision RECORDED in the manifest (leg 7). Legs 3/4/5/6 PASS on all expressible paths; leg 2 inexpressible under F-P2B-1.

## P5 merge-review LOW/INFO (reviewer agent-164, verdict MERGE-OK, 2026-08-13)
- LOW: auto_routing.rs:880-883 comment claims pin snapshots carry "budget 1", but journal_snapshot always writes attempt_budget: 3 — inert (plan_resume returns None for non-AutoClientSide) but the journaled record contradicts the documented intent. Fix: journal the true per-mode budget or correct the comment.
- LOW: §8.1's "500-with-format-body resolves terminal" holds only when body evidence reaches classify_attempt; the production wire fold extracts body evidence only for auth_error (flux_common.rs:69-90), so a live 500+format-body cascades. Consistent with probed-live-behavior discipline; untested at the adapter fold layer.
- INFO: exec auto_client_side refusal builds CandidateInputs with stubs (exec_run.rs:140-160) — safe today (requirements.tools forces empty admission); revisit when the tool-capability catalog lands.

## P3 fixround merge-review LOWs (reviewer agent-167, verdict MERGE-OK, 2026-08-13)
- LOW: is_https_url (mcp_specs.rs:39) checks only scheme+host — userinfo/query/non-default ports parse. Not exploitable today (HTTP registration typed-refused; origin_of rejects userinfo/query/fragment at login; host-granularity grant model). Tighten at parse time when the §6.1 HTTP binding lands.
- LOW (wording): "failed login journals nothing" holds only pre-grant — journal-first journals the grant at §6.3 step 3 before the browser handoff (flow.rs:222); an abandoned handoff leaves a token-less grant. Benign (no credentials → 401 → typed AuthorizationRequired on replay) but document the actual semantics.
- Integrator note: fixround merge had 2 trivial union conflicts (nano-cli/lib.rs + nano-session/lib.rs) resolved by keeping both sides.

## P3 leg-6 OAuth re-proof findings (agent-168, manifest section: CONSOLIDATED-VERIFICATION.md "P3 leg-6 OAuth re-proof (post-fixround @ 40aaf8d)")
- **F-LEG6R-1 LOW:** `mcp_specs.rs:29-31` vs `nano-egress/policy.rs:91-99` — `https:///mcp` accepted at parse (url crate normalizes host to `mcp`) but `allow_url`'s `host_of` doesn't arm it, so `auth login` dies `egress_denied` instead of the promised typed parse refusal for hostless URLs. Fail-closed, zero socket. Repro: capture `hostless_spec_login`.
- **F-LEG6R-2 ENVIRONMENT (host-bound):** this Windows host's Credential Manager refuses ALL generic-credential writes (`CredWrite` win32 8, reproduced with raw `cmdkey /generic`, VaultSvc RUNNING). `auth login`'s store step can never succeed here; product handles it exactly as designed (typed `McpCredstoreUnavailable`, no partial success). Populated-keyring login/logout live proof needs a host with a working vault.
- Re-proof verdict: legs 1-5 PASS (round-trip through token exchange via the unit battery's scripted-transport seam + real loopback sockets/journal/store; PKCE S256 cryptographically recomputed; journal-first grant observed durable; hostile-listener gauntlet bounded). Store step env-blocked per F-LEG6R-2.

## P4 adversarial proof findings (agent-169, manifest section: CONSOLIDATED-VERIFICATION.md "P4 adversarial proof (post-merge @ 3f9bf87)")
- CRITICAL/HIGH going to fix round (feat/p4-fixround): F-P4-1 (rule DSL wholesale unwired — engine green in nano-core but no gate arm/card options/session-load/CLI/journal op/error kinds; untracked in RC2-WIRING-PASS.md; HIGH live-proven inert), F-P4-2 (Windows ACL-audit stub execrules.rs:902-919 hard-Errs all rules.toml load/amend even though the P2a helper landed at attachment_store.rs:1067; HIGH probe-proven — rules dead on primary platform even post-wiring).
- **F-P4-3 MEDIUM (live-proven):** `session/load` from a second host SUCCEEDED while host 1 held the session mid-turn (slice-0 deviation D8 made concrete; captures/leg6b-live.json). Load-bearing deferral — two hosts can append-mark the same journal.
- **F-P4-4 LOW:** `cmd /c <denied> <trailing args>` degrades to Prompt floor instead of recovering Deny (fail-safe direction; never-Allow holds).
- New lows by reference (manifest leg-1/leg-8 lists): repo_map read-only `starts_with` prefix predicate; PTY cap check-then-insert race; pty_write unit divergence; review 2×10s worst case; review `.git` symlink pre-check; session/list params silently ignored; nested-mint stricter-than-note; TS/JS ASCII-only identifier extraction (INFO). Confirmed-still-open: F-31 LOW-5, F-37 LOW-2/3, F-29 LOW-3.
- TOCTOU live-growth race not deterministically reproducible on this host — `take(max+1)` bound code-verified (store.rs:359-370); documented gap.
- Review-mode legs ALL GREEN (leg 7) — this run is the §14 leg-2 live proof; nanoExtensions advertisement pin flip unblocked (integrator action).

## P5 adversarial proof findings (agent-172, manifest section: CONSOLIDATED-VERIFICATION.md "P5 adversarial proof (post-merge @ a3420ee)")
- Verdict: 10/13 PASS, 2 FAIL (both MEDIUM — filed here, no fix round per process), 1 PASS-with-definitive-negative. §6 provenance-only rule CANNOT lift: 16/16 live calls echo the requested alias; actual-leaf identity absent; buffered + SSE alike.
- **F-P5-1 MEDIUM:** 500-with-format-body cascades — §8.1 conflict class holds only in a pure-function unit fed a synthetic `body:` signal production never populates (`signals_of_model_error` hardcodes `body: None`, auto_routing.rs:566-620; `flux_common::classify_status` folds only auth_error, flux_common.rs:63-86). Repro: `.tmp/p5-proof/adv` `adv_500_with_format_body_must_be_terminal`.
- **F-P5-2 MEDIUM:** chained kill-resume budget leak — `journal_snapshot` hardcodes `attempt_budget: ATTEMPT_BUDGET` (auto_routing.rs:873); ACP resume re-journals through it (acp_mode.rs:2748-2755), so a second kill replays `3 − consumed-this-turn` instead of the true remainder (1+1+2 = 4 physical attempts across one logical routed turn). Repro: `adv_double_kill_budget_leak`.
- **F-P5-3 LOW:** failed-attempt usage capture seam-only (`failed_usage: None` hardcoded, auto_routing.rs:966-972); §6 "meter failed attempts" unwired in production. **FIXED at 6bc5b67 (2026-08-14, wave-2 fix lane):** the driver error path carries no usage (ModelError has no usage payload), so the ladder now journals an EXPLICIT record for every failed attempt — provider-reported failure usage retained verbatim when observed, else the honest zero shape (zero tokens, priced=false, reported=false; never fabricated). `RoutingUsage::to_turn_usage` folds the unreported all-zero record to nothing so the session sum is never mislabeled `estimated` over zero tokens. Pinned by `failed_attempt_without_wire_usage_journals_honest_zero_record`.
- **F-P5-4 LOW:** merged live leg-1 test writes its fixture one `..` too many (p5_auto_routing.rs:2531-2534 → outside workspace). Prover relocated the canary-clean fixture to shared/fixtures/flux/auto-routing/ and removed the stray tree; the test path bug remains.
- **F-P5-5 LOW:** pin/implicit turn frames drop the response-reported model (engine meters the configured reference, turn.rs:1121; no TurnEnd model field). Moot while alias-echo stands.
- Confirmed live, NOT a finding (design-§3-compliant known gap): production Auto refuses `capability_empty` pre-dispatch on tool-bearing turns (acp_mode.rs:2602 forces tools=true) until the tool-capability catalog lands.

## S1 auto-tools lane (feat/s1-auto-tools) — fixes and new findings
- **FIXED F-P5-1** (commit on this branch): `flux_common::classify_status` folds 5xx + `error.type=="invalid_request_error"` into the new typed `ModelError::InvalidRequest` (non-retryable, `ModelServer4xx` presentation); `signals_of_model_error` maps it to terminal `FormatRejected`. The leg-7 unit now derives the signal from the real adapter error, and the seam leg `adv_500_with_format_body_must_be_terminal` pins terminality + zero rung-2 calls.
- **FIXED F-P5-2** (same commit): `journal_snapshot` takes `attempt_budget` as a parameter; the ACP resume arm passes `plan_resume`'s true remainder. Seam leg `adv_double_kill_budget_leak` pins the 1+1+1 = 3 conservation across chained kills.
- **FIXED F-P5-4** (test-only): `live_leg1_alias_identity` and the new S1 live leg resolve the shared fixture root by ancestor walk (`shared_flux_fixture_dir`) — no hardcoded `..` count, correct from main checkout and worktrees.
- **F-S1-1 LOW:** `exec --auto --goal` is a typed usage refusal (exit 2) — goal-mode Auto needs a per-turn ladder (the §4 budget is per Auto turn); v1 builds the ladder for plain exec turns only. ACP goal-less and goal turns are unaffected (the ACP ladder is per-prompt by construction).
- **F-S1-2 LOW:** rung-2/rung-3 tool blessings are vacuous in v1 — production passes an empty `approved_leaves` manifest (panel Q3 unresolved), so no leaf probes were run and no leaf entries were flipped. Bless leaves only after the manifest channel lands.
- Alias-echo identity gap UNCHANGED by design: 3/3 S1 trials echoed `flux-auto` as response model; §6 alias metering stays provenance-only (unpriced). Capability admission is not identity attribution.
- Live kill-resume on a real flux-auto tool turn NOT run (judged not cheap: ACP process kill mid-stream + resume needs a live-driver host harness that does not exist; budget continuity is pinned at the seam by `adv_double_kill_budget_leak` and the journaled-remainder assertion).

## S2 vision-wire build (lane S2, branch feat/s2-vision-wire, 2026-08-14) — F-P2B-1 fix + deferred items
- **F-P2B-1 status: FIX LANDED on this branch, owner closes.** The vision_backed conjunct is refit (acp_mode.rs): Flux-provider leaves/aliases admit images on EITHER wire per the probe capture (shared/fixtures/flux/vision/flux-openai-wire/20260814_probe_capture.json) and the owner contract; the four flux aliases are blessed in the vendored vision catalog (proven → flux-openai-wire/20260814_manifest.json); the completions wire now carries image-bearing tool results as a trailing user message of base64 data-URI image_url parts (the RC2 `tool_result_images` refusal on flux-completions is removed; flux-responses keeps it — that wire is unreachable in production ACP). Contract constraints enforced at intake: one image per message (typed `image_too_many`), remote http(s) URLs typed-refused (never passed through), loader size caps unchanged.
- **F-P2B-4 (PDF) — OPEN; current authoritative status: pending live closure.** The implementation and live evidence commit `0eb5098426f95ee8d8e33bb4c35d370d399ea6b4` exist, with current external receipt SHA-256 `949a38c71320db0506ba9a2b1925d0d44bc993038c22ab15e44e7bf375635c50` and 1878 bytes, but the item does not close until active DEV-WP-0.3O Plan 08 validates the current seven evidence rows, external receipt, independent recheck, and closure summary. Historical context: PDFs require the Anthropic document block on POST /v1/messages; the OpenAI `file` block was blind against routing aliases. Close means the active O closure gate passes and the owner promotes the result.
- **F-P2B-5 (metering) — OPEN.** The 2026-08-14 probe showed usage.prompt_tokens does NOT include image tokens on the openai wire (image turn: 33 prompt tokens vs the 172-token text-only baseline — the image bytes are unmetered in usage). Attachment cost therefore CANNOT be recovered from usage metering; sessions re-send attachment bytes every turn (no Files API — full prompt tokens each time, per the owner contract). Close means: a client-side attachment cost model (bytes → estimated tokens) or Flux-side usage accounting of image tokens; until then cost reporting under-counts image-bearing turns.
- **F-P2B-6 (remote image URL fetch+inline) — OPEN, blocked on nano-egress.** The contract's preferred handling for remote URLs is fetch-in-harness-and-inline, but `EgressClient::fetch_bounded` content-type-gates to text/* + a few application types (client.rs ~353: image/* is ContentTypeDenied), and nano-egress is a fail-closed security invariant outside this lane's boundary. Shipped behavior: remote http(s) image URLs are typed-refused at ACP intake (never passed through). Close means: an egress image-fetch variant (image/* allowlist, bounded, private-range deny) + intake wiring + probes — or delete this entry if FluxRouter's planned edge normalisation (contract "What is changing" #3) lands first.
- **F-P2B-7 (multi-image messages) — OPEN, upstream-settled.** Contract rule 4 (one image per message) is enforced as a typed intake refusal because a Flux two-image probe miscounted ("1"). Revisit when FluxRouter settles multi-image support; lifting it = delete the count check in acp_mode.rs and re-probe.

## S3 session-ownership lane (F-P4-3 fix) — residual scope note (2026-08-14)

- **F-S3-1 LOW (scope boundary):** the S3 ownership lock covers the
  enumerated writer paths — acp-host session/new|load, exec (incl.
  --resume), and transitively goal/fork/cron writers via the SessionGuard
  OS layer. `protocol-host` (`crates/nano-cli/src/host_mode.rs:91`) opens
  its FIXED `protocol-host.jsonl` for writing with NO ownership lock: two
  protocol-host processes on one NANO_HOME can double-append. It can never
  collide with an ACP/exec session (distinct fixed id), and the lane
  boundary excluded it. Close means: acquire `SessionOwnership`
  (`session_guard_registry().try_own`) at protocol-host startup, fail
  closed with a typed busy when a second protocol-host is already running.

# Plugin substrate follow-ups (S8)

# S9 computer-use follow-ups

- Complete macOS CGEvent/CGDisplay dispatch and record the live TCC proof before advertising computer use.
- Complete Linux X11 XTest input/capture and run the two `xvfb-run` live legs before advertising computer use.
- Route Wayland `wlrctl`/`grim` fixed argv through a concrete `nano-platform` `SpawnSpec`; restricted compositors must remain unregistered.
- Complete Windows `type`, `key`, and `scroll` `SendInput` dispatch plus live focus-invariance, landing, and 100%/150% HiDPI evidence.
- Add the integrator-owned `nano-session` CUA journal/error/resume vocabulary and `nano-agent`/`nano-tools` approval, permission-mode, cancellation, panic-containment, attachment, and registration seams.
- Add the six-target cross-platform battery; capability flags remain false until each platform's live proof is recorded.

- Git-subprocess registry sources require an OS-containment and nano-egress design.
- ACP-host skill activation requires a skill-context consumer; protocol-host is the v1 consumer.
- HTTP-transport MCP plugin entries remain blocked on the §6.1 dispatcher HTTP binding.
- WASM/subprocess sandboxed plugins are outside the declarative v1 substrate.
- Manifest signing and verified publisher identity are post-v1 trust work.
- Paid marketplace accounts and account credential flows are post-v1.
- Add a real `plugin update` transaction; v1 update is remove plus freshly consented install.
- Add `${VAR}` environment indirection for MCP plugin specs without adding a credential channel.

## S10 soak harness follow-ups

- A loopback HTTP fake remains the preferred upgrade when the provider endpoint gains an override seam; the feature-gated in-process fake intentionally bypasses `nano-egress` and HTTP/stream parsing, as accepted by the locked S10 design.
- Run the mandatory 10 × 30-minute no-kill baseline set and one elevated Windows x64 eight-hour receipt before making a stable-gate claim. The local 60-second smoke is harness evidence only.
- Add a Unix owner receipt after the Windows gate receipt; Unix stays recommended rather than gate-blocking.

## Wave-2 fix lane (branch fix/w2-agent, 2026-08-14) — F-1, F-27 item 6, F-P3-5 FIXED on branch, owner closes

Per the adjudicated register in `shared/reviews/stable-wave/SEVERITY-SIGNOFF-2026-08-14.md`.

- **F-1 (SEV-1) FIXED.** Engine-side ceiling at the turn.rs history-append
  seam: `MAX_HISTORY_TOOL_RESULT_CHARS` = 128 KiB (above fs_read's 100 KB
  page so paged reads pass through; below shell's 256 KB/stream and the
  512 KiB MCP transport cap so oversized results compose down). Head+tail
  truncation with the visible typed marker `…[tool result truncated by the
  engine history cap: N chars in, M elided]…` — never silent clipping;
  char-based cut (no split UTF-8). Digest-first: the journaled
  `ToolResult.output_digest` is computed from the FULL raw output; ACP
  frames are built from those ops and carry digests only (test asserts no
  serialized op contains raw bytes). Tests (turn_tests.rs):
  `history_cap_boundaries_and_utf8_safety`,
  `oversized_tool_result_is_capped_marked_and_digest_journaled`,
  `under_cap_tool_result_flows_verbatim_unmarked`. Behavior note: a
  maxed-out shell result (>128 KiB combined) now reaches the model in the
  engine-capped marked form — intended consequence of the ceiling, not a
  regression.
- **F-27 item 6 (SEV-2) FIXED.** The mcp__ arm of
  `McpToolExecutor::execute_cancellable` routes through
  `McpClient::call_tool_cancellable` when the turn cancel flag is present
  (shared lock-to-clone resolution and result fold extracted into
  `resolve_mcp_call` / `execute_mcp` / `outcome_of_mcp`; the registry lock
  is never held across the wire wait — that half was already fixed).
  Test (mcp.rs integration_tests):
  `cancel_aborts_in_flight_mcp_call_end_to_end` — typed `user_cancelled`
  well before the server's 4s answer, the wire carries
  `notifications/cancelled` (server-side marker file), and the late
  response is dropped via the retired-id arm (a follow-up call on the same
  connection receives ITS answer, not the retired call's).
- **F-P3-5 (SEV-2) FIXED.** New `ToolExecutor::current_mcp_tool_definitions`
  hook (default `None`; `McpToolExecutor` answers from the live registry;
  every production wrapper — SessionTools, PtyToolExecutor, CronjobExecutor,
  TaskToolExecutor, MemoryToolExecutor, McpSessionToolExecutor,
  CheckpointToolExecutor, GoalToolExecutor — delegates). At each
  ModelRequest build, `TurnEngine::current_tool_definitions` splices the
  registry's CURRENT direct ∪ hydrated set in place of the
  construction-time `mcp__*` block (order preserved), so a journaled
  mid-turn `tool_search` hydration reaches the very next in-turn request.
  Derived from journaled state only (the `McpToolHydration` op lands
  durably before the registry mutates), so replay rebuilds the same set —
  asserted by rebuilding a fresh registry from the journaled op and
  comparing advertised names. Test (mcp_tests.rs):
  `mid_turn_hydration_reaches_next_in_turn_request`.

## F-41: tasks cancel-isolation test is timing-sensitive under suite-parallel load — LOW, test-robustness

- **Filed:** 2026-08-14, wave-2 lane observation. One full-suite run of
  `cargo test -p nano-agent` failed
  `tasks::tests::cancel_isolation_and_bounded_teardown_on_a_wedged_child`
  at tasks.rs:1982: the wedged child observed the cancel flag and exited
  `cancelled` BEFORE the bounded-teardown wait tripped, so the status never
  read `detached`. Green in isolation (5s run) and green on the immediate
  full-suite rerun (284/284); the lane's diff to tasks.rs is a purely
  additive trait-method delegation. Same failure class as F-38
  (host/scheduler-dependent), no product code implication observed.
- **Close means:** the test pins detach-vs-cancel ordering deterministically
  (e.g. hold the wedged child in a state that cannot observe the flag
  until after TEARDOWN_WAIT) without weakening the bounded-teardown
  assertion.

## Wave-2 sev-2 fix lane (fix/w2-mcp, 2026-08-14) — adjudicated items from shared/reviews/stable-wave/SEVERITY-SIGNOFF-2026-08-14.md
- **FIXED F-P3-6 at 67a81f9** (graceful close could lose `notifications/cancelled`): closing was set before the cancel sweep enqueued; the writer's closing-drain could quiet-exit first and the cancels died in the lane. The writer now holds its drain-exit until the sweep signals `cancels_queued` (dispatcher.rs). Pins: `dispatcher::tests::writer_holds_close_drain_until_cancels_queued` (deterministic) + `cancel_race_at_close_never_loses_the_cancel` (12-round fake-server battery).
- **FIXED F-P3-8 at cea3778** (>64 hydrated union bricked compaction): `hydration_carry_at` (nano-session/src/coordinator.rs) degrades an over-cap entry to digest/summary form — tool_names dropped whole (never a truncated subset), tools_digest + churn window carried. Resume re-exposes nothing for that server; tool_search re-hydrates. Compaction never bricks. Pin: `carry_degrades_when_hydrated_union_exceeds_the_name_cap` (70-name union, replay consistency, second compaction carries).
- **FIXED F-P3-11 at 697099d** (OAuth listener held ~180s after early failure): mechanism shipped with S5's ad87b4c (binding Drop cancels the accept loop); bind-before-DCR stands because registration needs the redirect_uri. This commit pins the flow-level evidence: `dcr_failure_releases_the_listener_port_promptly` recovers the bound port from the DCR request body and probes it released within 2s of the typed RegistrationFailed.
- **FIXED F-P3-12 at a48ead5** (unsanitized HTTP error surface): S5's 20be621 routed all `McpError::Transport` construction through `nano_egress::client::sanitize_transport_error` / status-only strings. This commit pins the redaction end-to-end: `http_error_surface_carries_no_body_or_credentials` (500 with a 64 KiB secret-marked body, garbage 200 body, connection-refused with query+userinfo markers, presented bearer) — none reach the error, text stays ≤ 256 chars. **Sev-1 check: NEGATIVE** — no credential material reaches model/card/log paths (AuthHeader values only ever become request headers; `resource_error_of_mcp` stringifies the sanitized text only).

## Wave-end audit fix lane (fix/wa-ckpt, 2026-08-14) — two High findings in crates/nano-checkpoints/src/lib.rs, FIXED on branch at 9991e7a, owner closes

- **F-42 (High, journal-claims-nonexistent-state ordering) FIXED.** `create_locked`
  journaled `CheckpointCreated` BEFORE the manifest/index were persisted; a crash
  in between left replay claiming an unreachable checkpoint. Now manifest+index
  are written and fsynced FIRST (`atomic_json` gained `sync_all` before the
  rename), and the journal append is the LAST step — a failed/torn final append
  loses only the claim while the ref-anchored commit, manifest, and index entry
  remain restorable truth. Restore path audited: `CheckpointRestoreBegin` lands
  before any workspace mutation and `CheckpointRestoreEnd` only after the apply
  completes (recovery re-applies an interrupted tail), so it already follows the
  same durable-first discipline — no change needed. Pin:
  `failed_final_append_leaves_durable_state_without_phantom_claim` (journal path
  torn mid-create: typed error, state listed+manifested, resumed journal carries
  no CheckpointCreated, and every landed claim resolves to a persisted index
  entry).
- **F-43 (High, 256 MiB cap didn't bound disk) FIXED.** Eviction removed only
  index entries; parent-chained `commit-tree` objects accumulated forever. Each
  checkpoint is now anchored under `refs/nano-checkpoints/<id>` (best-effort
  migration anchor for pre-existing stores at open); eviction deletes the ref
  and manifest, then runs a bounded `git gc --prune=now` (eviction-only, once
  per evicted batch); the cap is additionally enforced against the ACTUAL
  on-disk store size (`store_disk_bytes` over repo.git + manifests + index).
  Commits are deliberately parentless — ancestry chaining would keep evicted
  objects reachable from successors and unprunable; `CheckpointInfo.parent`
  remains journal/index metadata. All git subprocess calls stay on the crate's
  scrubbed/-c hooks-disabled discipline with argument-safe ref names. Pin:
  `eviction_prunes_object_store_below_cap` (40 checkpoints × 2 MiB
  incompressible payload: 32 retained, refs 1:1 with the index, evicted commits
  fail `cat-file -e`, disk < un-pruned accumulation and ≤ MAX_STORE_BYTES,
  newest checkpoint still restores). Behavioral notes: the just-created
  checkpoint is never evicted by its own create (a single >cap checkpoint is
  kept, not phantom-created); the disk-cap loop itself is only reachable when
  per-object overhead pushes disk past the logical sum, so the test exercises
  the count/logical eviction + pruning path and asserts against the cap rather
  than forcing 256 MiB of fixture data.
- **Found in testing, fixed in the same commit:** parentless commits with
  identical trees created within git's 1-second committer-timestamp granularity
  collapsed to the SAME commit id (previously kept distinct by the parent
  chain) — a shared id would let one index entry's eviction prune a sibling's
  objects. The commit message now carries a unique `now_nanos-sequence` token.

## Wave-end audit fix lane (fix/wa-bounds, 2026-08-14) — two High unbounded-allocation findings FIXED on branch, owner closes
- **FIXED MCP HTTP unbounded body read at 0b78d36** (`crates/nano-mcp/src/http.rs:98`): `Response::text()` buffered the entire server-controlled body before the status check / JSON-SSE parse. New `read_bounded_body` rejects a declared Content-Length over `MAX_HTTP_BODY_BYTES` (= `client::MAX_OUTPUT_BYTES`, 512 KiB — the protocol's output bound, matching the F-14 `read_error_body` philosophy at 64 KiB but sized for full responses) before any body byte is read, streams chunks into a capped buffer otherwise, and aborts with the typed `McpError::OutputBounded` (byte count only — the F-P3-12 error-surface discipline holds). SSE flows through the same `exchange`, so the event-stream path is bounded identically; the HTTP pump path inherits the bound unchanged. Pins: `http_declared_length_over_cap_aborts_typed_early`, `http_chunked_body_over_cap_aborts_typed_and_transport_recovers` (connection cleanup proven by a healthy follow-up round trip on the same transport), `http_sse_stream_over_cap_aborts_typed`, `http_chunked_sse_under_cap_parses` (the bounded path serves ordinary Flux framing).
- **FIXED hook stdout/stderr unbounded at f9cd8da** (`crates/nano-hooks/src/lib.rs:413`): both pipes used `read_to_end`, retaining everything a hook emitted until timeout. Both pipes now drain through `drain_pipe_capped` (`MAX_HOOK_OUTPUT_BYTES` = 1 MiB each); past the cap the drain keeps reading and discards — a full pipe never deadlocks the child — and an overshoot fails the hook with the new `HookOutcome::BoundedOutput`, distinct from `Timeout`. Windows Job-Object tree-kill on timeout is unchanged. The outcome maps to `nano_session::op::HookOutcome::BoundedOutput` (bootstrap/turn/compact emission sites); the journaled `HookDecision` stays digest-only. Older readers tolerate the new variant via the existing `serde(other)` Unknown fallback. Pin: `hook_output_over_cap_fails_bounded_output_without_deadlock` (> cap streamed from a target/-anchored fixture, BoundedOutput well before the 30s timeout, reason `hook output exceeded bound`, blocking_reason set).

## F-42: plugin skill roots have no discovery seam on exec/acp — LOW, activation parity

- **Filed:** 2026-08-14, wave-end audit fix lane (fix/wa-plugins, S8 inert
  plugins + silent downgrade). The lane wired installed-plugin MCP specs
  into ALL THREE bootstraps (host_mode.rs, exec_run.rs, acp_mode.rs — the
  same registry path as config-file servers) and plugin skill roots into
  the host_mode skill discovery, with fail-closed typed startup refusal on
  a corrupt plugin store (absent store resolves empty).
- **Gap:** plugin SKILL roots activate only on the protocol host. Exec mode
  has no skill-context assembly at all (no `prepare_skill_context` seam —
  exec builds turns from `v1_tool_definitions` + journal context), and the
  ACP host never discovers skills either (no skill block in its context
  assembly). Adding plugin roots there requires FIRST standing up skill
  discovery on those surfaces — a feature, not a surgical seam extension.
- **Close means:** if exec/acp ever gain skill discovery, their root lists
  must chain `plugin_cmds::plugin_skill_roots(nano_home)` with the same
  fail-closed Result discipline (corrupt store = typed startup refusal,
  never a silent zero).

## F-42: nano-cua live-desktop proofs (S9 §7.2) — capability flags stay FALSE until they land

- **Filed:** 2026-08-14, S9 completion lane (feat/s9-cua). The crate shipped
  headless-complete: policy battery, coordinate mapping, gate matrix, journal
  shapes, Wayland probe fixtures, redaction fixtures, and all four backends
  compile (Windows tested natively; macOS via `--target aarch64-apple-darwin`
  check+clippy; Linux via WSL test+clippy, both with and without the `x11`
  feature). Live dispatch is proven on NO platform: WSL on this host has no
  xvfb/X server, so even the CI-provable X11 leg ran self-skipped.
- **Gap:** the §7.2 battery in `crates/nano-cua/tests/live.rs` self-skips
  behind `NANO_CUA_LIVE=1` with reason strings. Until each platform's proof
  is run and recorded, `Capabilities.computer_use` stays FALSE per platform
  (honesty rule; the donor's `27-C2(b)` advertise-from-linkage defect is the
  anti-precedent). Do NOT wire advertisement at integration.
- **Close means:** per platform — Windows: focus-invariance + SendInput
  landing + HiDPI at 100%/150% on an interactive window station (owner-run);
  macOS: TCC-granted CGEvent/CGDisplay run on a logged-in GUI session;
  Linux X11: `xvfb-run cargo test -p nano-cua --test live` with
  `NANO_CUA_LIVE=1` (CI-automatable on the ubuntu legs); Linux Wayland: a
  live sway/river seat. Then, and only then, flip that platform's flag.

## F-43: reroute Wayland CUA helpers through nano-platform SpawnSpec

- **Filed:** 2026-08-14, S9 completion lane. nano-platform is a 5-line stub
  (no SpawnSpec exists), so `nano-cua/src/backends/linux_wayland.rs` shells
  out to `wlrctl`/`grim` directly in argv mode — the precedent S5 shipped in
  `nano-mcp/src/stdio.rs` (fixed program, separate argv entries, no shell).
  Design §2.6 prefers SpawnSpec routing once it exists.
- **Close means:** nano-platform lands SpawnSpec; the `run_argv` helper in
  linux_wayland.rs (and the `osascript`/`xdotool` frontmost probes) route
  through it, with the fixed-argv/no-model-interpolation contract kept.

## F-44: acp-host per-turn whole-journal rebuild was the 8h-soak memory creep — FIXED on fix/soak-mem-creep

- **Filed:** 2026-08-15, soak-memfix lane, from the S10 8h soak
  (run-20260814T180101592Z): acp-host PWS grew 28 → 218 MB over 7h and turn
  throughput decayed −49%. Root cause: after EVERY turn the ACP host re-read
  the whole session journal from disk (60 MB at h7) and rebuilt the context
  from all of history — `read_journal` whole-file read plus 4+ full passes
  (`SessionState::fold` for todos, `image_influenced_from_envelopes`,
  `journal_has_image_manifests`,
  `messages_from_envelopes_rehydrating`) per turn, then replaced
  `Session.context` wholesale.
- **Fix (this lane):** the journal → context fold is now INCREMENTAL. A
  carried `ContextFold` (`crates/nano-cli/src/acp_mode.rs`) advances between
  turns from a byte-offset tail read
  (`nano_session::reader::read_journal_from`, whole-line appends under the
  single-writer coordinator make delta reads race-free; an unterminated tail
  is left for the next read; any delta-read failure re-primes from ONE full
  read = the pre-fix behavior). `ContextFold::apply` is the ONE per-envelope
  reducer behind both the incremental path and the full-rebuild functions,
  so incremental == full rebuild byte-for-byte BY CONSTRUCTION — pinned by
  digest-equality tests across engine-driven turns, the full op vocabulary
  (images, steers, re-asks, todos, CUA pairs, compaction), a kill-resume
  re-prime, and a 200-turn session where each journaled byte is read exactly
  once. The journal stays the append-only authority: session/load keeps the
  ONE full read (kill-resume path unchanged; S9 ambiguous-tail semantics
  untouched). Chose the tail-read over folding the turn engine's op stream
  because the sink never observes session-tool appends (TodoSet) or
  cron/mode ops — the journal suffix is the only complete source, so
  equivalence holds by construction rather than by argument. The session no
  longer retains a materialized `context` Vec separate from the fold; the
  prompt's assembled context is built ONCE from the fold and moved into the
  engine (the per-prompt `active.context.clone()` deep copy is gone — the
  engine's owned-Vec hand-off is the one irreducible copy). Manual
  `session/compact` semantics preserved via `Session.context_override`;
  the S9 §4.2 resume block rides prompts until the first turn completes,
  the same point the old rebuild dropped it; the bounded prefix blocks are
  re-rendered at the SAME points the old rebuild used (session start +
  every turn completion, `Session.prefix_cache`), so F-C10-1's pinned
  one-turn-late AGENTS.md timing stands unchanged.
  `MeterState.samples` is now a bounded 64-sample windowed median (was an
  unbounded Vec).
- **Residual (accepted, documented):** `ContextFold` retains O(envelopes)
  auxiliaries (dedup id set, tool-call name pairing ≈ tens of bytes per op)
  for the session lifetime — bounded by op count, not journal bytes, and
  trivial beside the conversation itself. A live session's attachment
  degradation notice now fires once when the envelope folds instead of
  re-firing per turn (the fail-loud placeholder + session/update notice are
  unchanged; session/load re-derives and re-notices as before).
- **Close means:** owner verifies the next 8h soak: acp-host PWS slope ≈
  flat and no turn-throughput decay against the run-20260814T180101592Z
  baseline; then close.

## F-45: Residual ~8 KB/turn retained growth in the host turn path (S10 verification, sev-2)

- **Filed:** 2026-08-15, from the 1h verification soak on the leak-fixed
  binary (run-20260815T020556068Z) and two A/B runs.
- **Data:** after `798ecb0` (incremental fold) removed the per-turn full
  journal rebuild (200x fewer bytes read; throughput verified sustained
  5,445 turns/h, no decay), the host still retains ~8-10 KB/turn
  (~50 MB/h at max soak cadence): baseline 22.7 MB -> final 78.6 MB over
  1h/5,448 turns. A/B experiment (10-min runs, seed 777 vs 778): growth
  is IDENTICAL with and without `repo_map` calls (2->10 MB vs 2->11 MB)
  — the repomap tool/index is exonerated; the overhead is in the turn
  machinery itself (per-turn tool-definition rebuild, per-turn engine
  construction, or an accumulating registry — structure not yet
  identified; needs a heap profile, not more inference).
- **Bounds/mitigation:** harness absolute memory cap (1.5 GiB) passes
  with 12x headroom at receipt scale; compaction cycles and session
  restart bound it in practice. The 8h receipt (run-20260814T180101592Z)
  passes its budgets; the strengthened oracle (`0747ff7`) is what makes
  this residual visible.
- **Close means:** heap-profile the acp-host turn path under the soak
  workload, fix the retaining structure, and a 1h verification soak at
  max cadence shows slope <= 16 MiB/h (budgets.json, owner-locked).

## F-46: S4 hooks dead on the acp-host surface — FIXED at 85b5e2c

- **Filed:** 2026-08-15, post-stable audit: the S4 hook engine
  (`crates/nano-hooks`) only loaded in `crates/nano-cli/src/exec_mode.rs`
  and `crates/nano-cli/src/host_mode.rs`. The acp-host (Desktop + TUI — the
  primary product surface) never constructed a `HookEngine`, so a configured
  hooks.toml was inert exactly where it matters; the S4 lane had flagged the
  acp wiring as "integrator work" and it never landed.
- **Fix (this lane):** the engine is loaded ONCE per acp-host process from
  the same `<nano_home>/hooks.toml` source exec/host read (config under
  nano_home, TOML, command handlers only — a Desktop-run host and a CLI exec
  see identical hooks), carried on `ServeConfig::hooks`, and threaded into
  every session's TurnEngine through a new hooked
  `run_turn_streaming_with_context_blocks` entry
  (`crates/nano-agent/src/turn.rs`). Per-turn behavior now matches exec/host
  exactly: PreToolUse blocks AFTER approval, PostToolUse notify,
  UserPromptSubmit blocking, Stop one-continuation, PreCompact/PostCompact
  notify (auto path via the hooked engine; the manual `session/compact`
  path now calls `compact_messages_with_hooks` with trigger "manual").
  SessionStart fires at session/new ("startup") and session/load ("resume");
  SessionEnd fires best-effort at the three session-close points (host exit,
  session replaced by session/new, session replaced by session/load) — the
  S4 Drop-based SessionEnd in `bootstrap.rs` never landed on any surface, so
  acp is the first surface where it fires at all. Lifecycle decisions journal
  through the session's JournalCoordinator (P3 §3.3 one append authority,
  never the bootstrap lane's open-per-call second writer) with
  process-counter envelope ids (replay dedupes duplicate ids, so the
  bootstrap `{sid}-hook-{pid}-{index}` scheme would have silently dropped
  decisions across runs). Fail-closed preserved: a broken hooks.toml
  degrades to stderr warnings + zero hooks, never a dead host; blocking-hook
  failures still block per the S4 design.
- **Not wired (accepted, out of scope):** C6 child task turns
  (`TaskRegistry` builds child engines inside nano-agent) run hook-free on
  EVERY surface, exec/host included — parity, not a regression. The
  `bootstrap_session_with_hooks` / `HookedBootstrappedSession` seam remains
  unused dead code (exec/host bootstrap without it); the acp surface does
  not route through `bootstrap_session`, so the seam was left untouched
  rather than force-fit.
- **Evidence:** `crates/nano-cli/tests/s4_hooks_acp.rs` — wire-level battery
  over the real `acp_mode::serve` loop (scripted model, recording mock
  tools, real journal, real hook engine over a test hooks.toml): PreToolUse
  block denies after approval with the journaled HookDecision(Blocked) +
  `hook_blocked` ToolResult shape and the executor never runs; a read_only
  gate denial fires NO hook (after-approval ordering); notify hooks
  (SessionStart/UserPromptSubmit/PostToolUse) journal Pass while the turn
  completes; a resumed session fires SessionStart "resume" and blocks
  identically; a broken hooks.toml degrades to warnings. Gates green:
  `cargo fmt --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo test --workspace` (exit 0, no ACL env failures).
- **Close means:** closed by the fix; owner may verify on the next Desktop
  run with a hooks.toml installed.

## F-47: checkpoints shipped unreachable — the S7 integrator seam was never wired — FIXED

- **Filed:** 2026-08-15, from the post-stable audit: the S7 checkpoints
  engine merged but `checkpoint_tool_definitions()` /
  `CHECKPOINT_TOOL_NAMES` (crates/nano-agent/src/wiring.rs) and
  `CheckpointToolExecutor` (crates/nano-agent/src/checkpoint_tools.rs) had
  ZERO production callers — nothing in acp_mode.rs / exec_run.rs /
  host_mode.rs / the TUI registered them, while the stable gate claimed
  checkpoints work.
- **Fix:** 22d651d wires the S7 deviation-request seam on all three live
  surfaces (the TUI rides acp-host): per-turn tool-definition extend +
  `CheckpointToolExecutor` wrap beside the cronjob registration (acp), the
  same wrap on exec and protocol-host, the locked-design approval arms
  (acp arm 1h: create/list approve every mode, restore plan/read_only
  typed deny, default prompt, full_auto approve, always-prompt under the
  image-influenced clamp; exec: create/list approve, restore full_auto
  only and clamp-denied — the deviation request's predicted explicit arm,
  since exec's catch-all would have denied restore even in full_auto;
  protocol-host: restore denied under the plan posture), and the
  kill-mid-restore recovery sweep at every journal-open site (acp
  session/new + session/load, exec, protocol-host) via the shared
  `open_checkpoint_store`. A store that cannot open (gitless host,
  non-git-root workspace, busy lock) is a typed, loud skip that registers
  nothing — fail-closed, never a silent drop.
- **Evidence:** acp gate matrix (`s7_checkpoint_gate_matrix`), exec gate
  matrix + image clamp (`s7_exec_gate_checkpoint_matrix`), protocol-host
  posture arm
  (`plan_aware_approval_denies_checkpoint_restore_under_the_posture`),
  exec end-to-end create → modify → restore with the filesystem oracle
  (`s7_exec_checkpoint_create_modify_restore`), kill-mid-restore recovery
  via exec resume (`s7_exec_resume_recovers_interrupted_restore`), the
  child-surface pin inside the acp matrix; gate-all green (fmt check,
  clippy -D warnings, cargo test --workspace).
- **Close means:** closed by the fix commit; this entry is the audit
  trail.

## F-48: provision dry-run bin never prints its payload (raw-string literal)

- **Filed:** 2026-08-15, by the WP4 gate-card reconciliation lane.
- **Defect:** `crates/nano-sandbox/src/bin/provision_dry_run/main.rs:31` prints the
  launch line with a RAW string (`println!(r"...{b64}")`), so the `{b64}`
  placeholder is never interpolated — the base64 payload never reaches stdout.
  The WP4 provision gate re-encodes from the extracted JSON instead.
- **Severity:** SEV-3 (test-tooling surface; the provision gate works around it,
  but the bin's documented output contract is broken).
- **Close means:** interpolate (or drop) the placeholder and pin the output
  contract with a test.

## DEV-WP-0.4: frozen-contract tripwires cross the locked ownership boundary

- **Filed:** 2026-08-16, during WP-0.4 execution from `origin/master` at
  `7e47a10`.
- **Conflict:** the WP-0.4 goal card limits ownership to
  `shared/contracts/**`, `crates/nano-cli/src/bin/gen_contracts.rs`, one
  `justfile` recipe, and the owner-managed catalog entry. The authoritative
  hardening spec additionally requires modifying
  `crates/nano-session/src/op.rs` to add and exhaustively test
  `OP_VOCABULARY`, plus adding a workspace schema-validation test under
  `crates/nano-protocol`. Those required edits are outside the card's OWNS
  boundary.
- **Disposition:** WP-0.4 stopped before implementation; no partial contract
  artifacts or weaker substitute tripwires were created. Owner/integrator
  must expand the card's OWNS list to include the two required test/code
  surfaces, or amend the spec with an in-boundary derivation and test design.

## DEV-WP-0.4B: frozen-contract artifacts are outside the Git repository

- **Filed:** 2026-08-16, while executing plan `01-01` from baseline
  `10484c4` on `feat/wp-0.4`.
- **Conflict:** required `shared/contracts/*` files live outside the Git
  repository and no `shared/*` paths are tracked, so Tasks 2–3 cannot be
  committed atomically on the WP branch.
- **Disposition:** no product edits occurred. Provide a tracked shared
  repository/mount or authoritatively amend the ownership and commit contract.

## DEV-WP-0.2A: mem-stats feature requires the nano-cli package manifest

- **Filed:** 2026-08-16 while researching WP-0.2 from baseline `566e3ac` on
  `feat/wp-0.2`.
- **Conflict:** the WP requires a feature-gated `nano-cli/mem-stats` surface,
  but its OWNS list grants the root virtual `Cargo.toml` and not
  `crates/nano-cli/Cargo.toml`, where Cargo package features must be declared.
- **Disposition:** no product code or soak measurement was started. Authorize
  the exact package-manifest feature-table slice or amend the feature contract;
  the executor must not infer broader nano-cli manifest ownership.
- **Owner authorization (2026-08-16):** signed. Grant only the
  `crates/nano-cli/Cargo.toml` `mem-stats` feature-table slice; reporter
  configured-path failures fail startup; retain and force-add only exact
  canary-covered soak runs; select a suspect only at >=60% of positive
  accounted growth with a >=10 percentage-point lead, otherwise `neither`;
  interpret `sessions_map` as current `Option<Session>` cardinality 0/1; use
  B1 for WP acceptance while reporting fake-mode B11 separately.

## DEV-WP-0.2B: governed canary scanner cannot cover WP soak evidence

- **Filed:** 2026-08-16 during the WP-0.2 plan audit from baseline `566e3ac`.
- **Conflict:** every retained capture requires a real-key canary, but
  `scripts/canary/scan.mjs` scans only its fixed legacy target map and WP-0.2
  does not own that file. Credential-shape matching cannot substitute for the
  binding actual-key-in-memory comparison.
- **Disposition:** no soak was started. Authorize only an additive exact-file
  include-list option and coverage receipt in the governed scanner, or require
  an equivalent owner-run scanner step; never expose the key value.
- **Owner authorization (2026-08-16):** signed. Grant only additive
  `--include-list <exact-list.json> --receipt <exact-receipt.json>` behavior
  plus its synthetic self-test in `scripts/canary/scan.mjs`. Exact-list mode
  may compare the real key in memory, but must never print, persist, echo, or
  embed its value; default scanner behavior remains unchanged.

## WP-0.2 900-second profile failure (Plan 02-02)

- **Run:** `scripts/soak/evidence/run-20260816T161856444Z` from `4a53c86`.
- **Disposition:** the first wrapper attempt was aborted after roughly 42
  seconds and 8 turns (`exit 124`). It has no completed manifest or usable
  reporter series and is therefore `aborted/unclassified`: it selects no arm
  and cannot authorize a correction or one-hour receipt.
- **Canary:** the exact-list scanner failed closed because its required
  repo-local `.secrets/flux-test-key` path is absent in the isolated worktree.
  It emitted no receipt or secret value. The defense-in-depth
  `wp02-credential-shapes-v1` scan passed with zero hits but cannot substitute.
- **Corrected rerun:** `run-20260816T163631293Z` completed 901,636 ms and
  produced 57 reporter rows plus 15 aligned oracle samples across three PID
  segments. Exact-value canary passed over both retained attempts (12 files,
  269,655 bytes, zero hits). Eligible fold auxiliaries explained 28.094% of
  positive accounted growth and measured MCP registry growth was 0%; under
  the owner-signed 60%/10-point rule and independent evidence-review PASS,
  measured `neither` is confirmed. Plan 02-03 applied no product correction;
  F-45 remains OPEN. B1 and scaled B5 failed; no one-hour receipt ran.
- **Plan 02-04 eligibility:** INELIGIBLE/NO-RECEIPT. Because no measured
  correction landed, the 3,600-second B1 acceptance run was not started; B1
  and B11 acceptance are not evaluated or claimed. F-45 remains OPEN. No
  budget, harness, product, `.gitignore`, or evidence-staging change occurred.
- **Plan 02-05 audit/handoff:** one High was found and fixed in the single
  bounded round: exact-list paths were resolved from the linked-worktree
  parent instead of the current worktree root. Exact-list mode now uses the
  worktree root while legacy default coverage keeps its historical root;
  synthetic resolver/inventory tests and focused suites pass. `just gate-all`
  (including `gate-gen-check`) and opt-in reporter/release gates pass. Final
  status remains measured-neither, F-45 OPEN, no 3,600-second receipt.

## DEV-WP-0.3A: PDF journal/store authority boundary — RESOLVED

- **Boundary deviation/resolution (2026-08-17):** the one-round WP-0.3 audit found
  that the required additive `DocumentRef` journal serde/digest contract and the
  attachment-store reachability/retention/orphan/malformed-reference battery crossed
  the original OWNS list (`SPEC-WP0-hardening.md` WP-0.3 type-plumbing requirement and
  `03-RESEARCH.md` journal/store test evidence). Owner authority is now narrowed to
  `op.rs` for only the additive `DocumentRef`/`InputBlock::DocumentRef` contract
  comments/tests and `attachment_store.rs` for only production `DocumentRef` journal
  reachability plus retention/orphan/malformed-reference handling and their comments/tests;
  store redesign, `ImageRef` rename, `ToolResult` changes, and broader session work
  remain excluded. The audit also resolved generator authority: the tracked in-repo
  nano-session contracts JSON and shared/contracts mirror remain mandatory, while any
  sibling desktop mirrors are optional generator-only owner/integrator refreshes and
  cannot satisfy standalone CI/DoD or be committed by the WP branch.

## DEV-WP-0.3B: canonical Flux Anthropic catalog/provenance authority — RESOLVED

- **Boundary deviation/resolution (2026-08-17):** the mandatory active
  `flux-router-anthropic` runtime leaf cannot obtain endpoint/wire authority from
  `WAYLAND_NANO_PROVIDERS`; that payload selects only a canonical provider/model/key
  state. WP-0.3 narrowly owns the vendored catalog JSON, the provider-catalog
  `RECORDED_SHA256` and exact endpoint/scope assertions, the build.rs-generated golden,
  and one exact `UPSTREAM.md` ledger/endpoint-review row. No other catalog, test,
  golden, build-script, routing, or provenance edit is authorized.
### DEV-WP-0.3C — D9 generator allowlist omission (RESOLVED 2026-08-17)

Plan 03-02 referenced and executed `crates/nano-cli/src/bin/gen_error_table.rs`, and the authoritative WP-0.3 GOALS/spec explicitly own that generator, but its `files_modified` entry was accidentally omitted when the D9 exact allowlist was frozen. A bounded review fix correctly made the mandatory canonical shared mirror fail closed, then D9 stopped on the omission before commit. The integrator recorded this deviation and repaired only the Plan 03-02 declaration plus the matching exact `wp03_control_v1.repo_allowlist` entry; no broader ownership was granted, and the full D9 check must pass before the product change is committed.
### DEV-WP-0.3D — Responses codec exhaustiveness guard (RESOLVED 2026-08-17)

Adding the provider-neutral `ContentBlock::Document` made the existing exhaustive Flux Responses encoder fail to compile. The authoritative WP-0.3 boundary owns `crates/nano-model/**`, but Plan 03-03 had omitted `crates/nano-model/src/flux_responses.rs`. A focused decision audit rejected silent filtering because it would recreate the proven blind-answer failure. The narrow resolution adds only an explicit unreachable pre-dispatch invariant guard plus its local regression test; the canonical typed `ModelLacksPdf` zero-call refusal remains owned by Plan 03-05. The integrator added exactly this file to Plan 03-03 and the D9 allowlist; no codec redesign or broader routing authority was granted.
### DEV-WP-0.3E — Document request-byte accounting exhaustiveness (RESOLVED 2026-08-17)

The additive `ContentBlock::Document` also made the existing exhaustive request-size heuristic in `crates/nano-agent/src/compact.rs` fail to compile. The authoritative WP-0.3 boundary owns `crates/nano-agent/**`, but Plan 03-03 omitted this consumer. The narrow resolution adds exact document base64 byte-length accounting plus its local monotonic regression test, matching the heuristic's purpose without changing compaction policy. The integrator added only this file to Plan 03-03 and the D9 allowlist; no unrelated compact/fold behavior was authorized.

### DEV-WP-0.3F — Non-self-referential evidence manifest (RESOLVED 2026-08-17)

The evidence manifest cannot contain its own final hash and byte count. It now records exactly six paired payloads; the scanner and receipt treat the current manifest as the seventh file externally. The receipt excludes itself and validates all seven current hashes and byte counts. No product ownership or runtime behavior changes.

### DEV-WP-0.3H — Product-fix versus lifecycle metadata history (RESOLVED 2026-08-17)

Immutable history places the audit artifact and plan summaries between audited product bytes and product-fix commits. The v2 audit now records product fixes separately from an exact ordered lifecycle-metadata chain: only enumerated audit/summary paths may intervene, while `fix.final_commit/final_tree` always identify final product bytes. This is a documentation/schema correction only and grants no broader commit or path allowance.

The independent recheck additionally records a distinct committed `recheck_point`; its lifecycle chain must be complete from the final product commit to that point. Recheck command receipts are created only by the detached execution loop at the final product commit/tree, every Critical/High verdict reference resolves to exactly one of those receipts, and the recorded command array must match the executed receipts exactly. Closure independently recomputes the full history, product-byte identity, and finding verdicts, and binds the canonical `flux-router-anthropic:flux-auto` Anthropic Messages endpoint facts to its exact successful detached provider-catalog test receipt.

The proof is executable rather than self-reported: Plan 11 derives the exact Git revision interval to its captured pre-output recheck point and runs focused commands in a disposable detached worktree; Plan 08 independently derives the complete audited-to-closure history and reruns required commands against the detached final product commit.

### DEV-WP-0.3G — D9 bounded hashing parallelism (RESOLVED 2026-08-17)

D9 now bounds manifest hashing workers at the smallest of 16, half the logical processor count, and the file count. The unchanged exact `-Mode Check` baseline comparison passed in 850.655 seconds on a 32-processor host, about 109 seconds (11.4%) below the prior typical 960 seconds; enumeration, hashes, sorting, schema, and failure semantics remain unchanged.

### DEV-WP-0.3I — Lifecycle phase-boundary reconciliation (SUPERSEDED historical; resolved 2026-08-17)

Historical record only; DEV-WP-0.3O is the sole active authority.

The planning contract now separates immutable audit history, post-fix lifecycle metadata, independent-recheck artifact receipts, and later live-evidence phase history. Plan 13 captures the actual clean pre-output HEAD/tree as its input tip and projects every non-product commit through that tip, preserving `85a8b1d91379243aebd23ee74bc190221b670563` as an ordered anchor rather than a terminal hash; its JSON may describe only history known at the captured tip, and its summary records only P13A. Plan 11 derives the complete post-fix chain through actual P13S from Git, creates audit-only P11A, and its summary records only P11A. Plan 07 discovers actual P11S from Git and its summary records only pre-summary live evidence. Plan 08 discovers both summary commits directly from Git and validates their one-file diffs. No summary records its own commit/tree. Detached receipts normalize command ID, exit zero, product commit/tree, test name, pass marker, and `detached-worktree` mode; raw timing/output and temporary paths are not persisted or compared. No product files are changed by this resolution.

### DEV-WP-0.3J — Exact PDF recheck selection (SUPERSEDED historical; resolved 2026-08-17)

Historical record only; DEV-WP-0.3O is the sole active authority.

The prior short filter could exit successfully after selecting zero tests. Canonical receipt command 2 now names `acp_mode::tests::pdf_actual_serve_pinned_auto_and_compatible_dispatch_are_recorded` fully and passes `--exact --nocapture`; both recheck and closure require transient proof of exactly one executed test and one pass, and reject zero-test output. Its normalized receipt uses the same exact test ID.

P13S remains immutable at `0c13d7d` with P13A as its parent. Plan 11 discovers both from Git, proves their consecutive one-file diffs, and treats later documentation correction commits through the actual recheck HEAD as an exact ordered suffix. That suffix is limited to `03-11-PLAN.md`, `03-08-PLAN.md`, `03-VALIDATION.md`, `SOURCE-AUDIT.md`, and `docs/FOLLOWUPS.md`; audit JSON, summaries, and product paths remain forbidden. This is a PS5.1-safe planning correction only.
### DEV-WP-0.3K — Canonical shared evidence path mapping (RESOLVED 2026-08-17)

The D9 initializer originally copied the repository spelling `crates/nano-model/fixtures-flux/pdf/**` into the external shared allowlist, but the authoritative paired mirror is `shared/fixtures/flux/pdf/**`. The live fixture therefore could not be admitted by the exact shared-delta gate. The initializer and frozen control now map only that seven-file prefix to `fixtures/flux/pdf/**`; the receipt already used the canonical path. No additional shared path or product ownership was granted.
### DEV-WP-0.3L — Shared evidence directory manifest nodes (RESOLVED 2026-08-17)

### DEV-WP-0.3M — Second final-fix independent recheck (SUPERSEDED historical; resolved 2026-08-17)

Historical record only; DEV-WP-0.3O is the sole active authority.

The canonical final fix is `4fd669bfb921769456f1603221bbe2326487d67c` (tree `84af3ddd0d0773bc72db7684c516a622bd4453c4`). Plans 11 and 08 validate the exact 18-commit audit history, both product-fix projections and all four findings, then keep post-fix metadata, the non-self P11 output boundary, and later live evidence as separate exact Git segments. The final detached catalog has four deterministic receipts: endpoint, fully-qualified PDF refusal, fully-qualified nano-protocol Windows verbatim regression, and nano-model clippy. Test receipts require one passed/no zero tests; clippy requires a stable clean completion. High 001 binds to PDF, High 002 to Windows, and High 003/004 to clippy. No product or evidence path is admitted into metadata, no raw command output is persisted, and no Plan 13 terminal assumption remains.

## DEV-WP-0.3O: Canonical final-tree recheck and live-evidence segmentation — ACTIVE AUTHORITY; planning correction RESOLVED, live closure PENDING

Canonical product identity is now `f1372da6336f7bacad95b2c460c7f9ff1d4fcaf5`, tree `5ff1ea037d604c273095b5303062a68e936d83df`. Plans 11 and 08 validate the exact 25-commit audit projection, the exact product-fix commits `18d57a6`, `4fd669b`, and `f1372da` with their paths, and the union of all six findings. Findings bind 001 to PDF refusal, 002 to Windows verbatim, 003/004 to strict nano-model clippy, 005 to the `/v1` endpoint test, and 006 to the exact `pdf_evidence_manifest_schema_has_exact_six_payload_pairs` one-test receipt.

The post-fix chain is derived generically. `phase_history.post_fix` is exactly `2a55eae` documentation followed by `0eb5098` with seven live-evidence files; `c0d6f69` is later audit-only metadata. Current evidence hashes and external receipt metadata are validated independently from the f137 product tree. Post-recheck Plan 11 output/summary and Plan 07 summary closure remain separate Git segments. The final detached catalog has five actual-execution receipts, and no obsolete fix-count limit or lifecycle-plan terminal special case remains.

The canonical `shared/fixtures/flux/pdf` leaf did not exist when the D9 baseline was frozen, while its `fixtures` and `fixtures/flux` ancestors already existed. Creating the first owned fixture therefore adds exactly the `fixtures/flux/pdf` directory record to the full `{path,type,sha256,bytes}` manifest. D9 admits only that new directory node in addition to the already enumerated files; no sibling directory or additional file is allowed.
### DEV-WP-0.3N — Authoritative Flux PDF endpoint (RESOLVED 2026-08-17)

The GOALS/spec and owner-recorded 2026-08-14 media contract require `POST /v1/messages`, but research D5 accidentally pinned the older `/anthropic/v1/messages` compatibility path. Live path-only probes showed the compatibility path returned 200 with a zero document-token delta and no oracle, while `/v1/messages` produced a 13,831-token delta and the exact oracle on the same PDF block. Catalog authority, adapter default, generated golden, SHA pin, tests, provenance and closure checks now use `/v1/messages`; historical compatibility fixtures remain historical evidence rather than production authority.
