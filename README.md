# Wayland Nano — Track B (master implementation)

Greenfield + vendored-crate implementation of the shared Wayland Nano
contracts. Track B is the master implementation and carries the
product name (renamed from the NanoK3 codename — see `docs/REBRAND.md`).
Companion/competing implementation to Track A (`../nano/`, Codex reduction
fork — read-only from here). Comparison rules: `../shared/SCORECARD.md`.

Wayland Nano is a Rust workspace (edition 2024) implementing the Nano runtime:
an agent loop, a policy-enforced tool set, an OS-containment sandbox, an
egress chokepoint, and protocol hosts that let Wayland Desktop drive it.

- Constitution and boundaries: `ARCHITECTURE.md`
- Provenance ledger (every ported file): `UPSTREAM.md`
- Current sprint state: `docs/STATUS.md`
- Compliance scenario catalog: `docs/compliance/SCENARIO_CATALOG.md`
- Shared contracts/fixtures: `../shared/`
- Flux live evidence: `../shared/fixtures/flux/FINDINGS.md`

## Crate layout

| Crate | Role |
|---|---|
| `nano-core` | Shared types: absolute paths, permission profiles, policy engine |
| `nano-platform` | The OS boundary trait — the agent loop never sees OS details |
| `nano-sandbox` | OS containment: Windows restricted-token/DACL/Job/WFP, macOS seatbelt, Linux landlock/seccomp/bwrap (ported from Codex, see `UPSTREAM.md`) |
| `nano-egress` | Deny-by-default outbound HTTP chokepoint; `flux_only` preset; redaction-proof errors |
| `nano-model` | Provider-neutral model types, SSE parser, retry policy, Flux Completions client (v1 wire) |
| `nano-tools` | Policy-enforced fs/search/shell tools — all shell execution goes through the sandbox |
| `nano-agent` | Agent loop: turn state machine, loop protection, MCP routing, skills activation |
| `nano-mcp` | MCP client (stdio + HTTP/SSE) with Flux quirks handled |
| `nano-skills` | Skill loader/parser: bounded scoped activation, malformed skills surface as errors |
| `nano-session` | Append-only Op journal: replay, torn-tail recovery, idempotence, compaction equivalence |
| `nano-protocol` | NDJSON wire codec, honest capability profile, host loop, ACP adapter, Desktop corpus replay |
| `nano-cli` | The `wayland-nano` binary and helper bins |

## Build, test, gate

Toolchain is pinned by `rust-toolchain.toml`: **Rust 1.95.0**, native MSVC
(`x86_64-pc-windows-msvc`). Windows x64 is release-blocking; unix targets are
check/clippy-gated, runtime-proven on hosted runners.

```sh
just gate-all          # fmt check + clippy -D warnings + full test suite (the local gate)
cargo test --workspace # unit + integration tests, no live/networked tests
just gate-deny         # license/advisory gate (cargo deny)
just gate-release      # release binaries for packaging/metrics
just gate-live         # live Flux/MCP/slice tests — needs the key in env, never in CI
```

Live-gated tests self-skip without credentials (`FLUX_API_KEY`,
`FLUX_TEST_KEY`, or the file named by `FLUX_API_KEY_FILE`); a credential-less
run stays green. Clippy runs with `-D warnings` and enforces the architecture
bans (all outbound HTTP must flow through `nano-egress`).

## The `wayland-nano` binary

Three modes (`crates/nano-cli/src/main.rs`):

- `wayland-nano doctor` — self-diagnostics: environment, sandbox state,
  egress policy, journal integrity, process hygiene. Exit 0 when all required
  checks pass (unprovisioned sandbox is a WARN until elevated setup runs).
- `wayland-nano protocol-host` — Desktop-core NDJSON wire host: ready-first
  loop, malformed input → typed error frame + continue.
- `wayland-nano acp-host` — ACP adapter for Desktop registration
  (`acp.customAgents`): initialize v1 → session/new → prompt with streamed
  updates → end_turn.

Helper binaries: `wayland-nano-acp-profile` (metrics),
`wayland-nano-sandbox-setup`, `wayland-nano-command-runner`,
`wayland-nano-provision-dry-run`, `wayland-nano-tree-kill-probe`,
`wayland-nano-spawn-profile` (nano-sandbox, Windows),
`wayland-nano-linux-sandbox` (landlock/seccomp/bwrap helper, Linux).

## Install (npm alpha)

`packaging/npm/` ships a zero-dependency package with prebuilt binaries —
alpha, `private: true`, unsigned, not published to the public registry.
See `packaging/npm/README.md`:

```powershell
pwsh packaging/npm/scripts/pack.ps1   # build release + stage binary
cd packaging/npm; npm pack
npm install -g ./waylandnano-nano-0.1.0-alpha.0.tgz
wayland-nano doctor
```

Acceptance evidence: `packaging/npm/ACCEPTANCE.md` (recorded under the old
NanoK3 codename — offline clean-prefix install, doctor exit 0, ACP
initialize, unsupported-platform refusal).

## Security model

Two layers, fail-closed everywhere (`SANDBOX_UNAVAILABLE`, never silent
downgrade):

- **Egress chokepoint** — every outbound request flows through `nano-egress`,
  deny-by-default; the `flux_only` preset allows Flux hosts and denies the
  rest. Error `Display` is redaction-proof.
- **OS containment** — child processes spawn inside the sandbox
  (restricted token + deny-write DACLs + Job-object tree kill + WFP network
  block on Windows; seatbelt on macOS; landlock/seccomp/bwrap on Linux).
  Write outside the workspace root is denied end-to-end; descendants are dead
  within the cleanup deadline.
- **Policy-enforced tools** — fs/search tools enforce read/write roots,
  sensitive-file denies, and bounded reads; the shell tool only executes
  through the sandbox path.
- **Credential hygiene** — the Flux key resolves from env or a key file
  (`FLUX_API_KEY_FILE`); it never appears in config blobs, frames, or logs
  (canary-asserted in the vertical slice).

Never weaken any of the above — or a test exercising it — to make a run pass.
A failing test that exposes a real hole is a valuable result; report it.

## Status

C1 code-complete (C1.2 full proof awaiting owner elevated provisioning);
C2 + C3 claims posted to `../shared/reviews/`. ~290 workspace tests green,
clippy `-D warnings` clean. Details: `docs/STATUS.md`.
