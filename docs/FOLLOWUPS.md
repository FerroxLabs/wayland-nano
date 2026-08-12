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
