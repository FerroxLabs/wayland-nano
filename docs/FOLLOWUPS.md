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

## F-2: Desktop legacy ACP stack drops error code/data (AcpConnection.ts)

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
