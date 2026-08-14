# Wayland Nano — Desktop handoff: budget notices moved to ext notifications

Companion to `MAC-HANDOFF.md`. This document is the authoritative reply on
the P1 cost-metering wire change and the standing items adjudicated during
the Mac bring-up. Source of truth for every frame shape below is
`crates/nano-protocol/src/acp.rs` (emitters) and
`crates/nano-tui/src/acp_client.rs` (consumer-side parse) — if this doc and
the code ever disagree, the code wins.

## 1. What changed and why

Nano's P1 cost-metering notices previously rode `session/update` with custom
`sessionUpdate` kinds (`budget`, `budget_warn`, `budget_clamp`). Desktop's
finding — verified on our side — is that `@agentclientprotocol/sdk@0.18.2`
validates every `session/update` notification against the
`zSessionNotification` zod schema **before dispatch** and drops unknown
`sessionUpdate` kinds with `-32602`, so metering never reached the UI. The
fix moves all three notices to ACP ext notifications
(`_wayland/session/*`), which the SDK routes through `extNotification`
without schema validation. Field names are unchanged — only the frame shape
moved. Merged to master at `822ec44` (merge of
`fix/budget-ext-notifications`, payload commit `1853a5e`), CI green.

## 2. The contract

Three ext-notification methods. `params` is **flattened** — `sessionId` plus
the payload fields at the top level, **no `update` wrapper** (that wrapper
only ever existed because the old frames were `session/update`).

| Method | params | Fires |
| --- | --- | --- |
| `_wayland/session/budget` | `{sessionId, session_tokens, microcents, priced, limit, observed}` | Session meter status payload: totals + honesty label + cap position. `limit`/`observed` are `null` when no cap is configured. |
| `_wayland/session/budget-warn` | `{sessionId, limit, observed, pct_used}` | Once per 80% cap crossing, latest-wins (C7 vocabulary). |
| `_wayland/session/budget-clamp` | `{sessionId, requested, granted}` | A request's `max_tokens` was clamped to the reserved output allowance. Logged, never silent. |

All numerics are `u64`. `microcents` is the meter's cost figure in
microcents; `priced` is the honesty flag.

## 3. What Desktop must do

- Route the three methods above in the `extNotification` handler (or
  wherever the SDK surfaces ext notifications). They are notifications —
  no `id`, no response.
- Parse `params` as flattened fields (see table). Do not look for an
  `update` object.
- **Honesty rule:** `priced: false` means the cost figure is not real.
  It MUST render as `unpriced` — never as `$0.000`. A rendered zero on an
  unpriced turn is a lie we will treat as an integration bug.

## 4. Honest caveats

- The `_wayland/session/budget` frame is proven live over stdio against
  real Flux turns (capture exists in the repo evidence trail).
- `budget-warn` and `budget-clamp` were **not** triggered by the live
  smoke — they fire only on an 80% crossing or an actual clamp. Same code
  path as the live-proven budget frame, unit-tested, but not
  live-exercised. If the first real crossing misbehaves, that is why.
- Defense in depth: keep the `session/update` handler tolerant of unknown
  kinds (as you said you would), in case anything else custom ever leaks
  back onto that channel.

## 5. Standing notes (adjudicated Mac bring-up items)

One authoritative reply per item, so there is a single document of record:

- **Install location.** `~/.local/bin` is accepted for dev mode (Desktop
  dev inherits the shell PATH). `/usr/local/bin` is still required when
  Desktop is GUI-launched or packaged. If Nano ever ships bundled inside
  Desktop, the bundling question is yours to answer.
- **"Empty conversation history"** — withdrawn by Desktop (inactive-tab
  harness artifact). Closed.
- **Error-table consumer wiring (59 kinds)** and the **bundled-bun
  fresh-worktree ENOENT** are Desktop-owned. Our side of the error table is
  generated and complete (PR #954 merged, PR #955 pending on your branch).
- **`promptCapabilities.image` now advertises from the startup leaf
  (F-P2B-1 fix landed on the S2 lane, 2026-08-14).** The vendored vision
  catalog blesses the four flux routing aliases
  (flux-auto/standard/fast/reasoning) plus the previously-proven
  flux-pinned-* leaves, per the owner Flux media contract
  (shared/reviews/stable-wave/flux-media-contract-2026-08-14.md) and the
  local probe capture
  (shared/fixtures/flux/vision/flux-openai-wire/20260814_probe_capture.json).
  With the default startup leaf (`flux-auto`) the initialize response
  carries `image: true` and Desktop MAY send image blocks — inline base64
  (`{"type":"image","data","mimeType"}`), ONE image per prompt, never
  remote URLs (typed refusal otherwise). PDFs remain unsupported (tracked
  in docs/FOLLOWUPS.md).

## 6. What we need back from you

1. Confirm the ext-notification wiring lands on your side (all three
   methods routed, flattened params parsed).
2. Report any frame-shape mismatch observed against a real turn — quote
   the raw frame.
3. Confirm whether you want the error-table i18n decision scheduled, and
   if so who owns the slot.
