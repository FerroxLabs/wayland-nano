# Flux live endpoint verification — 2026-08-09 (Track B / K3)

Credential: owner-provided test key (never stored here; `.secrets/` local-only).
All fixtures contain request/response **bodies only** — no auth headers.

## Verdict: all six BUILD_PLAN_V3 DoD endpoints exist and answer

| Endpoint | Status | Evidence dir | Notes |
|---|---|---|---|
| `GET /v1/models` | **200** | `models/` | Full catalog: tier aliases + pinned models with token ceilings |
| `POST /v1/chat/completions` | **200** | `chat-completions/` | Real completion; `usage.cost_usd` present |
| `POST /anthropic/v1/messages` | **200** | `anthropic-messages/` | Native Anthropic envelope |
| `POST /anthropic/v1/messages/count_tokens` | **200** | `anthropic-count-tokens/` | `{"input_tokens":14}` — matches the messages call's input count |
| `POST /v1/responses` | **200** | `responses/` | Valid Responses object with reasoning items |
| `POST /mcp/` | **200** | `mcp/` | MCP `initialize` OK; server = `litellm-mcp-server` v1.0.0 |

This resolves K3-review amendment #1: `count_tokens` and `/mcp` were
"unverified" — both are now fixture-proven. Codex's six-endpoint DoD stands.

## Quirks discovered (must become adapter tests)

1. **Alias routing differs per surface.** `flux-auto` via `/anthropic` routed to
   `kimi-k2.6`; `flux-fast` via `/v1/responses` routed to `deepseek-v4-flash`.
   The same alias is not the same upstream model across wires.
2. **Reasoning eats tiny budgets → empty visible output.**
   - completions: `finish_reason:"length"`, `reasoning_tokens:14` of 16, content "ok"
   - messages: `stop_reason:"max_tokens"` with **empty text block**
   - responses: `status:"incomplete"`, output contains only a `reasoning` item
   Adapters must not treat empty visible output as an error, and must budget
   headroom above reasoning consumption.
3. **`/mcp` (no trailing slash) → 401** with a misleading "Ensure Key has
   `Bearer ` prefix" JSON-RPC error (even when Bearer was sent). **`/mcp/`
   (trailing slash) → 200.** Endpoint derivation must append the slash.
4. **`/mcp/` speaks SSE framing** (`event: message\ndata: {...}`) for responses,
   per streamable-HTTP MCP; server capabilities: prompts, resources(subscribe),
   tools.
5. Completions responses carry `reasoning_content` on the message and detailed
   usage (`prompt_cache_hit_tokens`, `cost_usd`). Anthropic-surface usage
   carries `cache_creation_input_tokens`/`cache_read_input_tokens`.
6. Auth: openai/responses surfaces accept `Authorization: Bearer`; anthropic
   surface accepts `x-api-key` (per Desktop, Bearer also accepted); `/mcp/`
   accepts `Authorization: Bearer`.

## Not yet covered (next fixture batches)

- Streaming (SSE) on all three inference wires; tool calls (parallel/partial);
  thinking-block and `cache_control` pass-through fidelity on `/anthropic`;
  `x-wl-*`/`x-flux-*` header behavior; 402/409 typed errors; `Retry-After`;
  cancellation mid-stream; `/mcp/` `tools/list` + `tools/call`; omit-`max_tokens`
  behavior for unknown models on completions.

---

## Batch 2 — 2026-08-09 (B-FLX-02, script: nano-k3/scripts/flux-probe/batch2.sh)

### Streaming: all three wires canonical
- completions: 25+ `chat.completion.chunk` frames; carries inline
  `reasoning_content` deltas (deepseek-style) — adapters must separate it
  from `content` deltas.
- anthropic: canonical lifecycle `message_start → content_block_start →
  content_block_delta×N → content_block_stop → message_delta → message_stop`.
- responses: canonical lifecycle `response.created → in_progress →
  output_item.added → reasoning_summary_text.delta×N → output_text.delta×N →
  … → response.completed`.

### Tool calls: work on both inference wires
- completions: proper `finish_reason:"tool_calls"` + `tool_calls[]` (id/args).
- anthropic: proper `tool_use` content block + `stop_reason:"tool_use"`.
- **Translation artifact:** anthropic-surface `tool_use.id` is `call_*`
  (OpenAI-style), not `toolu_*` — the surface is a translation layer, not a
  native Anthropic passthrough.

### WIRE-2 GATE VERDICT (plan v3 §5): thinking/cache pass-through FAILS
- `thinking:{type:"enabled",budget_tokens:1024}` on `/anthropic` → accepted
  without error but **no thinking block in response**, on BOTH `flux-auto`
  (routed qwen-plus) AND **`flux-pinned-claude-sonnet`** (routed
  `claude-sonnet-5` — a real Claude). Thinking is silently dropped.
- `cache_control:{type:"ephemeral"}` on a system block →
  `cache_creation_input_tokens:0` on every call; no explicit cache write
  recorded. (One call showed `cache_read_input_tokens:128` — ambient infra
  caching, not request-driven.)
- **Consequence: the plan-v3 rationale for preferring the Messages wire
  (native thinking/caching) does not hold against live Flux. Recommendation:
  Chat Completions as the single production wire for v1; Messages adapter
  exists for compatibility but is not the preferred route.** Both tracks
  should adopt this unless a later Flux release changes the behavior
  (re-probe then; fixtures make re-verification cheap).

### Alias routing rotates (fixture determinism warning)
`flux-auto` served by kimi-k2.6 / qwen-plus / MiniMax-M3 across identical
requests (input token counts varied 620–764 for identical bodies). Use
`flux-pinned-*` models for deterministic wire-fidelity fixtures.

### omit-`max_tokens` on completions: CONFIRMED
No field → `finish_reason:"stop"`, no 400, `service_tier:"standard"` present.
The #456/#462 contract holds on the completions wire.

### `/mcp/` tools/list: endpoint + session work, catalog is EMPTY
`initialize` → session id via `Mcp-Session-Id` header; `tools/list` →
`{"tools":[]}`. Discovery/capability negotiation proven; nothing to invoke
as of this probe. DoD "Flux MCP invoke" currently has no invocable tool.

---

## Batch 3 — 2026-08-10 (G-FLX-2, script: nano-k3/scripts/flux-probe/batch3.sh)

Evidence dirs: `errors/`, `rate-limit/`, `cancel/`, `headers/`. Batch 3 also
records **response headers** (never request/auth headers — verified clean);
earlier batches were bodies-only. Covers the batch-1 open items "402 typed
errors, Retry-After, cancellation mid-stream, header behavior".

### (a) 402 entitlement-exceeded: NOT reproducible — over-limit returns 413
`max_tokens:10000000` on completions → **HTTP 413 Payload Too Large**, typed
JSON body (`errors/*_cc_overlimit_response.json`):
`error.message="context_window_exceeded"`,
`provider_specific_fields.reason` = "Request needs ~10000008 tokens … exceeds
the context window of every model eligible for this tier",
`required_tokens:10000008`. The test key (`x-litellm-key-max-budget` ≈ $1B)
cannot be driven over entitlement, so 402 stays unverified; 413 is the live
"request too big" shape.

### (a2) Substitution probe — invalid key returns 500, NOT 401 (MAJOR)
Bad key → **HTTP 500** (`errors/*_cc_badkey_response.json`):
`{"error":{"message":"Flux Router error: Authentication Error, Invalid proxy
server token passed. key=<sha256-of-presented-key>, not found in db. …",
"type":"auth_error","code":"500"}}`.
**Credential-hygiene note:** the server embeds the SHA-256 of the *presented*
key in the error body. The fixture contains only the hash of the fake probe
key `sk-invalid-nanok3-probe-…` (verified: sha256 matches the fake key, not
the real one) — but a real-but-rejected key's hash would land in any log
that records the error message.

### (b) 429 / Retry-After: NOT observable — bursts saturate the edge with 503
- 40 sequential requests → all 200 (`rate-limit/*_cc_burst_statusline.txt`).
- 80 parallel → 57×200 / 23×503; 120 parallel → 73×200 / 47×503. **Zero 429**
  at any feasible load (`rate-limit/*_cc_parallel_burst_*`).
- The 503 is a bare **edge nginx HTML page** (`Content-Type: text/html`,
  non-JSON) with **no `Retry-After` header** — and no `Retry-After` /
  `x-ratelimit-*` header appears on ANY response, including 200s.
- Live failure mode under load is therefore retryable 5xx, not 429.

### (c) Mid-stream cancel: clean teardown, no server error frame
Streaming completion aborted client-side after 2048 bytes (~7 chunks, mid
`reasoning_content`; `cancel/*_cc_stream_partial.txt`). Client-side write
error `curl: (23)` on downstream close (`*_cc_stream_cancel_stderr.txt`);
partial ends mid-chunk with no terminal `[DONE]` and **no server-injected
error event** — the server simply streams until the socket closes. Treating a
truncated SSE stream as a transport/cancel condition (not a protocol error)
matches live behavior.

### (d) Response-header inventory: no `x-wl-*`; actual families are `x-flux-*`, `x-litellm-*`, `llm_provider-*`
Full capture: `headers/*_cc_inventory_headers.txt`. On a normal completion:
- **`x-flux-*`** (router telemetry): `routed-model`, `model`,
  `original-model`, `routed:true`, `request-id`, `cost-usd`, `available`,
  `held`, `loop-engaged`, `phase-latency` (JSON per-phase ms),
  `summarization-applied`, `tier-escalated`, `model-window`,
  `engines-applied` (e.g. `r10_11_rtk_compactor,r10_2_semantic_cache,…`).
- **`x-litellm-*`** (proxy telemetry): `call-id`, `model-id`, `version`,
  `response-cost` (+original/discount/margin), `key-max-budget`, `key-spend`,
  `response-duration-ms`, `overhead/callback-duration-ms`, `model-group`,
  `attempted-retries`, `attempted-fallbacks`,
  **`model-api-base` — leaks the upstream provider URL**
  (`https://api.deepseek.com/v1`).
- **`llm_provider-*`**: full upstream response headers mirrored verbatim
  (trace ids, CloudFront pop, CORS, …).
- **No `x-wl-*` headers exist** (plan docs anticipated them; real prefix is
  `x-flux-*`). No `x-ratelimit-*` on success responses.
- Info-leak note: `model-api-base` + `key-spend`/`key-max-budget` are
  upstream/account telemetry — do not log response headers wholesale.

### ⚠ Contradictions vs nano-model retry/classify (flagged, NOT fixed here)
1. **Auth failure = 500, not 401.** `classify_status`
   (`nano-k3/crates/nano-model/src/flux_completions.rs:107`) maps 401|403 →
   `Auth`, 500 → `Server`; `is_retryable` (`src/retry.rs:30`) retries all
   5xx. Live behavior means an invalid/expired key is **retried ~6× with
   backoff and then surfaces as `Server`, never `Auth`** — the classifier's
   central assumption is inverted by the live wire.
2. **Context overflow = 413, not 400.** The `400 if message.contains
   ("context"|"token")` → `ContextOverflow` arm never fires for the real
   shape; live 413 falls through to `Server{413}` (non-retryable, but wrong
   variant — callers cannot detect context overflow).
3. **`Retry-After` is doubly dead.** Flux never sends it (503 HTML instead
   of 429), and `classify_status` only receives `(status, body)` — it could
   not populate `RateLimited.retry_after_ms` from a header even if one
   arrived; the "Retry-After honored first" path in `retry.rs` is
   unexercised against live Flux.
4. **`x-wl-*` header assumptions** (if any adapter/probe looks for them)
   must target `x-flux-*` instead.

These are recordings + flags only — no model-code changes in this batch.
