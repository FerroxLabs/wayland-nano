# External Integrations

**Analysis Date:** 2026-08-16

## APIs & External Services

**Model providers:**
- Flux Router - Primary exercised OpenAI-compatible completions, Responses, Anthropic-compatible Messages, token counting, model catalog, and Flux MCP service.
  - SDK/Client: internal clients in `wayland-nano/crates/nano-model/` and `wayland-nano/crates/nano-mcp/` over Reqwest 0.12 through `wayland-nano/crates/nano-egress/`.
  - Endpoint authority: `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json` (`https://api.fluxrouter.ai`); evidence/status is recorded in `wayland-nano/docs/COMPATIBILITY.md` and `shared/fixtures/flux/`.
  - Auth: `FLUX_API_KEY`, then live-test fallback `FLUX_TEST_KEY`, then the file named by `FLUX_API_KEY_FILE`, implemented in `wayland-nano/crates/nano-cli/src/flux_key.rs`.
- Catalog providers - Anthropic, OpenAI, OpenRouter, Groq, Mistral, DeepSeek, Together, Fireworks, Perplexity, Cohere, Cerebras, xAI, Moonshot, NVIDIA, MiniMax, and Google Gemini.
  - SDK/Client: provider-neutral OpenAI-completions or Anthropic-messages clients in `wayland-nano/crates/nano-model/`; exact wire, base URL, API path, credential variable, and proof flag are embedded in `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json`.
  - Auth: injected `WAYLAND_NANO_OAUTH_BEARER_<PROVIDER>` with optional expiry metadata, then the catalog's canonical API-key env var, then `<VAR>_FILE`; use `wayland-nano/crates/nano-cli/src/provider_key.rs`.
  - Proof constraint: treat each catalog row's `proven` flag as authoritative; NVIDIA and MiniMax are present but unproven in the current catalog.

**Web search:**
- Flux search - First backend in the resolver chain when Flux credentials resolve; implemented in `wayland-nano/crates/nano-tools/src/web_search.rs`.
  - SDK/Client: internal OpenAI-compatible client via `nano-model`/`nano-egress`.
  - Auth: Flux credential chain from `wayland-nano/crates/nano-cli/src/flux_key.rs`.
- Brave Search API - Second search backend at `api.search.brave.com`, implemented by `BraveSearchClient` in `wayland-nano/crates/nano-tools/src/web_search.rs`.
  - SDK/Client: internal Reqwest/egress client, not a vendor SDK.
  - Auth: `BRAVE_SEARCH_API_KEY`.
- Tavily Search API - Third search backend at `api.tavily.com`, implemented by `TavilySearchClient` in `wayland-nano/crates/nano-tools/src/web_search.rs`.
  - SDK/Client: internal Reqwest/egress client, not a vendor SDK.
  - Auth: `TAVILY_API_KEY`.

**MCP:**
- Remote MCP servers - JSON-RPC MCP client with HTTP/SSE and stdio transports in `wayland-nano/crates/nano-mcp/`; protocol version is `2025-06-18` in `wayland-nano/crates/nano-mcp/src/protocol.rs`.
  - SDK/Client: internal `nano-mcp` implementation; HTTP uses Reqwest through `nano-egress`, while stdio child processes are contained by `nano-sandbox`.
  - Auth: optional OAuth 2 authorization-code + PKCE S256 flow in `wayland-nano/crates/nano-mcp/src/oauth/`.
- Flux MCP - Remote endpoint under Flux `/mcp/`; handshake/list is implemented and verified, but the upstream tools catalog is currently empty, so no invocable Flux MCP tool is claimed (`wayland-nano/docs/COMPATIBILITY.md`).

**Desktop host protocols:**
- Wayland Desktop - Drives the runtime via ready-first NDJSON protocol host or ACP adapter in `wayland-nano/crates/nano-protocol/` and `wayland-nano/crates/nano-cli/src/main.rs`.
  - SDK/Client: internal codecs and host loops; shared contracts and fixtures live under `shared/contracts/` and `shared/fixtures/`.
  - Auth: Desktop may inject short-lived provider bearer credentials using the environment contract consumed by `wayland-nano/crates/nano-cli/src/provider_key.rs`; Nano does not own provider refresh-token storage.

## Data Storage

**Databases:**
- Not detected. The active implementation uses local files rather than an external database.
  - Connection: Not applicable.
  - Client: Rust standard filesystem APIs and typed journal modules.

**File Storage:**
- Append-only JSONL session journals, replay, locking, migration, and compaction are implemented in `wayland-nano/crates/nano-session/`; the journal is the durable source of truth for resumable state.
- Local plugin/package archives and caches are managed by `wayland-nano/crates/nano-plugins/`; archive integrity uses SHA-256 and bounded tar/gzip handling.
- Shared recorded evidence and integration fixtures live under `shared/fixtures/`; treat these as test evidence, not production mutable storage.
- npm binaries and integrity metadata are staged by `wayland-nano/packaging/npm/scripts/pack.ps1`; generated `packaging/npm/binaries/` content is ignored and must not be committed.

**Caching:**
- Flux model discovery uses a fetched `GET /v1/models` result with fixture fallback in `wayland-nano/crates/nano-model/src/flux_models.rs`.
- No external cache service is used; cache and replay state remain local and journal-authoritative under `wayland-nano/crates/nano-session/`.

## Authentication & Identity

**Auth Provider:**
- Model-provider API keys or Desktop-injected OAuth bearers.
  - Implementation: resolve credentials only through `wayland-nano/crates/nano-cli/src/provider_key.rs`; provider endpoint/env-var definitions come only from `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json`.
- MCP OAuth 2 authorization-code with PKCE S256.
  - Implementation: discovery, loopback callback, token exchange/refresh, bounded errors, and storage live in `wayland-nano/crates/nano-mcp/src/oauth/`; plain PKCE is rejected rather than downgraded.
  - Storage: Windows Credential Manager uses the service namespace `wayland-nano MCP` through the pinned `windows-sys` shim in `wayland-nano/crates/nano-mcp/src/oauth/wincred.rs`. Headless/unix refresh-token fallback uses the file named by `NANO_MCP_OAUTH_REFRESH_FILE_<SERVER>` with owner-only permissions in `wayland-nano/crates/nano-mcp/src/oauth/storage.rs`.
- OS sandbox identities.
  - Implementation: Windows uses namespaced `NanoSandbox*` local identities provisioned by `wayland-nano/scripts/provision/`; sandbox and egress must fail closed when unavailable (`wayland-nano/AGENTS.md`).

## Monitoring & Observability

**Error Tracking:**
- No external error-tracking service detected.
- Typed failures are surfaced through the closed `NanoErrorKind` vocabulary in `wayland-nano/crates/nano-session/src/error_kind.rs` and protocol presentation in `wayland-nano/crates/nano-protocol/`.

**Logs:**
- Daily rolling local sandbox logs with bounded retention are implemented using `tracing-appender` in `wayland-nano/crates/nano-sandbox/src/logging.rs`; command previews are sanitized and debug logging is opt-in via `SBX_DEBUG=1`.
- Metrics use a structured-log sink rather than a hosted telemetry backend in `wayland-nano/crates/nano-sandbox/src/telemetry.rs`; global telemetry settings default to none.
- CI produces machine-readable gate, performance, and CycloneDX evidence artifacts in `wayland-nano/.github/workflows/gate.yml`.

## CI/CD & Deployment

**Hosting:**
- Public npm registry for the `waylandnano` wrapper package, configured in `wayland-nano/packaging/npm/package.json` and published by `wayland-nano/.github/workflows/release.yml`.
- GitHub Releases for per-platform native zip archives, checksums, and Desktop-consumed release assets in `wayland-nano/.github/workflows/release.yml`.
- Runtime hosting is local/native: the npm wrapper selects and verifies a bundled binary; there is no server deployment target (`wayland-nano/packaging/npm/bin/install.js`).

**CI Pipeline:**
- GitHub Actions gate on pushes to `master` and pull requests across six OS/architecture legs in `wayland-nano/.github/workflows/gate.yml`.
- Gates include rustfmt, Clippy with warnings denied, workspace tests, cargo-deny, platform helper/runtime checks, advisory performance evidence, and a CycloneDX SBOM.
- Tag-driven `v*` release builds five npm platforms, publishes npm provenance through Sigstore/OIDC, attests archives with SLSA build provenance, and uploads GitHub release assets in `wayland-nano/.github/workflows/release.yml`.

## Environment Configuration

**Required env vars:**
- No credential is universally required for offline gates; live-gated tests self-skip without credentials (`wayland-nano/justfile`).
- Flux: `FLUX_API_KEY`, `FLUX_TEST_KEY`, or `FLUX_API_KEY_FILE` (`wayland-nano/crates/nano-cli/src/flux_key.rs`).
- Catalog providers: the canonical API key variables listed in `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json` (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `GROQ_API_KEY`, `MISTRAL_API_KEY`, `DEEPSEEK_API_KEY`, `TOGETHER_API_KEY`, `FIREWORKS_API_KEY`, `PERPLEXITY_API_KEY`, `COHERE_API_KEY`, `CEREBRAS_API_KEY`, `XAI_API_KEY`, `MOONSHOT_API_KEY`, `NVIDIA_API_KEY`, `MINIMAX_API_KEY`, `GEMINI_API_KEY`) or each corresponding `<VAR>_FILE`.
- Search: `BRAVE_SEARCH_API_KEY` and `TAVILY_API_KEY` enable their fallback backends in `wayland-nano/crates/nano-tools/src/web_search.rs`.
- MCP OAuth refresh fallback: `NANO_MCP_OAUTH_REFRESH_FILE_<SERVER>` in `wayland-nano/crates/nano-mcp/src/oauth/storage.rs`.
- Release only: repository secrets `NPM_TOKEN` and GitHub-provided `GITHUB_TOKEN`; references occur in `wayland-nano/.github/workflows/release.yml`.

**Secrets location:**
- Production/provider secrets are supplied through process environment, Desktop-injected bearer variables, OS Credential Manager, or explicitly named owner-only key files. Never store values in repository configuration; the resolution boundary is `wayland-nano/crates/nano-cli/src/provider_key.rs`.
- The owner-held Flux live-test key file exists outside the active source tree at the path documented by `wayland-nano/AGENTS.md`; reference its path only and never read or copy it.
- GitHub Actions repository secrets supply release credentials in `wayland-nano/.github/workflows/release.yml`.

## Webhooks & Callbacks

**Incoming:**
- MCP OAuth uses a loopback authorization callback listener with bounded timeout and state/PKCE validation in `wayland-nano/crates/nano-mcp/src/oauth/`.
- Local Desktop communication enters through stdin/stdout NDJSON (`protocol-host`) or ACP (`acp-host`) in `wayland-nano/crates/nano-cli/src/main.rs`; these are process protocols, not public webhooks.
- No public inbound HTTP webhook endpoint detected.

**Outgoing:**
- HTTPS calls to the selected model provider endpoint from `wayland-nano/crates/nano-model/data/providerCatalog.vendored.json`, always through `wayland-nano/crates/nano-egress/`.
- HTTPS calls to Brave or Tavily search from `wayland-nano/crates/nano-tools/src/web_search.rs` when their credentials resolve.
- HTTP/SSE MCP requests and OAuth discovery/token requests from `wayland-nano/crates/nano-mcp/`; endpoint grants are method/path scoped and redirect handling is fail-closed through `wayland-nano/crates/nano-egress/`.
- npm publish, GitHub release creation, artifact upload, and provenance attestation occur only in `wayland-nano/.github/workflows/release.yml`.

---

*Integration audit: 2026-08-16*
