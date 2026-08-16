# Wayland Nano — Flux Endpoint Contract (v1)

**FROZEN v1.0 — 2026-08-11**
Change control: changes require owner sign-off plus an evidence update (new
or re-recorded fixtures under `shared/fixtures/flux/` in the same change).
Descriptive-first: every shape below is **as recorded against live Flux**
(batches 1–3, 2026-08-09/10 — `shared/fixtures/flux/FINDINGS.md`). Fixtures
contain request/response bodies only (batch 3 also response headers), never
auth headers. Anchors SCORECARD C1.1. Scenario IDs per
`wayland-nano/docs/compliance/SCENARIO_CATALOG.md` §C.

## 1. Endpoint inventory (all six live-verified, 200)

| Endpoint | Fixtures | Notes (recorded) |
|---|---|---|
| `GET /v1/models` | `models/` | Full catalog: tier aliases + pinned models with token ceilings (COMP-FLX-001) |
| `POST /v1/chat/completions` | `chat-completions/` | **v1 production wire** (§3); `usage.cost_usd` present (COMP-FLX-002) |
| `POST /anthropic/v1/messages` | `anthropic-messages/` | Translation layer, not native passthrough: `tool_use.id` is `call_*`, not `toolu_*` (COMP-FLX-003) |
| `POST /anthropic/v1/messages/count_tokens` | `anthropic-count-tokens/` | Matches messages input count (COMP-FLX-004) |
| `POST /v1/responses` | `responses/` | Valid Responses object with reasoning items (COMP-FLX-005) |
| `POST /mcp/` | `mcp/` | MCP `initialize` OK (`litellm-mcp-server` v1.0.0). **Trailing slash required** — `/mcp` → misleading 401. SSE framing per streamable-HTTP; session id via `Mcp-Session-Id` header; `tools/list` works but the catalog is **empty** (COMP-FLX-006/011) |

Auth (recorded): openai/responses surfaces accept `Authorization: Bearer`;
anthropic surface accepts `x-api-key` (Bearer also accepted); `/mcp/` accepts
`Authorization: Bearer`.

Streaming is canonical on all three inference wires; tool calls work on both
inference wires (`streaming/`, `tool-calls/` — COMP-FLX-007/008). Omitting
`max_tokens` on completions is confirmed safe: `finish_reason:"stop"`, no
400, `service_tier` present (`omit-max-tokens/` — COMP-FLX-010).

## 2. Recorded quirks (normative for adapters)

1. **Alias routing differs per surface and rotates.** The same alias is not
   the same upstream model across wires; `flux-auto` was served by
   kimi-k2.6 / qwen-plus / MiniMax-M3 across identical requests. Use
   `flux-pinned-*` for deterministic wire-fidelity fixtures.
2. **Reasoning eats tiny budgets → empty visible output** (completions
   `finish_reason:"length"`; messages empty text block; responses
   `status:"incomplete"` with only a reasoning item). Empty visible output is
   not an error.
3. Completions carry `reasoning_content` on the message and detailed usage
   (`prompt_cache_hit_tokens`, `cost_usd`); streaming chunks carry inline
   `reasoning_content` deltas that adapters must separate from `content`.
4. Mid-stream cancel: the server streams until the socket closes — partial
   stream ends mid-chunk with no `[DONE]` and no server error frame
   (`cancel/`). A truncated SSE stream is a transport/cancel condition, not a
   protocol error.

## 3. The Completions-as-v1-wire decision (wire-2 gate, recorded FAIL)

Wire-2 gate (COMP-FLX-009, fixtures `thinking/`, `cache/`):
`thinking:{type:"enabled",budget_tokens:1024}` on `/anthropic` is accepted
without error but **no thinking block comes back** — on `flux-auto` AND on
`flux-pinned-claude-sonnet` (a real Claude). `cache_control` produces
`cache_creation_input_tokens:0` on every call. Thinking/cache pass-through
is **silently dropped** by live Flux.

**Decision: Chat Completions is the single production wire for v1.** The
Messages adapter is compatibility-only and not the preferred route. This
stands unless a later Flux release changes the behavior (re-probe; fixtures
make re-verification cheap). Client proof: `nano-model` Completions client
with offline fixture replay green (COMP-MODEL-001/002/003) and live smoke
(COMP-MODEL-007).

## 4. Error classification (live-recorded truth → typed mapping)

Implemented in `wayland-nano/crates/nano-model/src/flux_completions.rs:107-139`
(`classify_status`), corrected against batch-3 fixtures and proven by
`fixture_tests.rs` (COMP-MODEL-004 + batch-3 replay tests):

| Live shape (recorded) | Typed class | Retryable | Evidence |
|---|---|---|---|
| HTTP 500 with `error.type=="auth_error"` (invalid/expired key — **never 401**; body embeds `key=<sha256-of-presented-key>`, a digest not the key) | `Auth` | no | `errors/*_cc_badkey_response.json`; `batch3_badkey_500_auth_error_classifies_as_auth_not_retryable` |
| HTTP 401 / 403 | `Auth` | no | spec arm, retained (`flux_completions.rs:117`) |
| HTTP 413 `context_window_exceeded` (the live "request too big" shape; over-limit `max_tokens` → 413, **not** 402) | `ContextOverflow` | no | `errors/*_cc_overlimit_response.json`; `batch3_overlimit_413_classifies_as_context_overflow` |
| HTTP 402 entitlement | `Entitlement` | no | spec arm only — **unverifiable with the test key** (`x-litellm-key-max-budget` ≈ $1B); documented substitution (FINDINGS batch 3 §a) |
| HTTP 429 | `RateLimited` | yes (Retry-After if present) | spec arm only — **never observed live** (FINDINGS batch 3 §b) |
| HTTP 503 bare **edge nginx HTML** (non-JSON, no `Retry-After`) under burst load | `Server{503}` | yes | `rate-limit/*_cc_parallel_burst_*`; `batch3_burst_503_edge_html_classifies_as_retryable_server` |
| other 5xx | `Server{status}` | yes | `classify_status` fallthrough |

Recorded load behavior: 40 sequential → all 200; 80 parallel → 57×200/23×503;
120 parallel → 73×200/47×503. Zero 429 at any feasible load. The live failure
mode under load is retryable 5xx, not 429.

## 5. Header families (recorded response headers, batch 3 `headers/`)

- **`x-flux-*`** (router telemetry): `routed-model`, `model`,
  `original-model`, `routed`, `request-id`, `cost-usd`, `available`, `held`,
  `loop-engaged`, `phase-latency`, `summarization-applied`,
  `tier-escalated`, `model-window`, `engines-applied`.
- **`x-litellm-*`** (proxy telemetry): `call-id`, `model-id`, `version`,
  `response-cost`, `key-max-budget`, `key-spend`, `response-duration-ms`,
  `model-group`, `attempted-retries`, `attempted-fallbacks`, and
  **`model-api-base` — leaks the upstream provider URL**.
- **`llm_provider-*`**: upstream response headers mirrored verbatim.
- **No `x-wl-*` headers exist** (plan docs anticipated them; real prefix is
  `x-flux-*`). No `Retry-After` / `x-ratelimit-*` on ANY response, including
  200s.
- Info-leak rule: `model-api-base`, `key-spend`, `key-max-budget` are
  upstream/account telemetry — **do not log response headers wholesale**.

## 6. Retry policy (implementation facts)

`wayland-nano/crates/nano-model/src/retry.rs` (COMP-MODEL-006): max 6 attempts;
exponential backoff 500ms ×2 capped at 32s with 25% jitter; `Retry-After`
honored first **when present** (live Flux never sends it — the 429 path is
spec compliance, unexercised live); retryable set = `RateLimited`,
`Server{≥500}`, `Transport`; `Auth`, `ContextOverflow`, `Entitlement`,
`Cancelled` give up immediately.

## Machine-readable authority

`contracts/flux-endpoint-contract.json` is the canonical machine-readable sibling.
Hand-frozen from the six recorded fixture-backed endpoints in §1; changes follow the frozen change-control rule above and require matching evidence updates.

