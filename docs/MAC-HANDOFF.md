# Wayland Nano — Mac build & Desktop integration handoff

**Read this first if you have zero context.** This document assumes the
machine (and the person/agent driving it) has never seen Wayland Nano.

## 0. What this is

**Wayland Nano** is a first-party, sandboxed coding agent written in Rust
(repo: `FerroxLabs/wayland-nano`). It speaks **ACP** (Agent Client Protocol)
natively over stdio — the same protocol Wayland Desktop uses to talk to
external CLI agents (Codex, Grok, Kimi, etc.). Nano needs no bridge: Desktop
spawns `wayland-nano acp-host` and streams JSON-RPC.

**Wayland Desktop** (repo: `FerroxLabs/wayland`) is the Electron app. Its
`feature/wayland-nano` branch already contains complete first-class Nano
integration: an always-listed `wnano` preset, typed-error rendering, and
session UI. **No Desktop code changes are needed on the Mac** — the only
Desktop-side work left is merging one generated-file PR (#954, error table).

State of the code: `wayland-nano` master @ `8a2c3ce`, CI 6/6 green. All RC2
capability packs merged and adversarially proven: web search + cost metering
(P1), image intake (P2a), MCP/ToolSearch/OAuth (P3), image tool results
(P2b), exec rules/review mode/PTY/repomap/session browser (P4), Flux Auto
routing (P5).

## 1. Prerequisites on the Mac

```bash
xcode-select --install                 # C toolchain for Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # rustup
brew install just bun                  # gate runner + Desktop package manager
```

- Rust: the Nano repo pins **1.95.0** in `rust-toolchain.toml`; rustup
  auto-installs/selects it on first cargo command. Do not override.
- macOS 14+ (Apple Silicon is the CI-proven target; Intel macOS 15 is also
  CI-gated).
- GitHub access: both repos are private under FerroxLabs — `gh auth login`
  or an SSH key with access.

## 2. Build Wayland Nano

```bash
git clone https://github.com/FerroxLabs/wayland-nano.git
cd wayland-nano
cargo build --release
```

Binary lands at `target/release/wayland-nano`. Smoke checks:

```bash
./target/release/wayland-nano --version   # prints version, exit 0
./target/release/wayland-nano             # bare run prints usage, exit 2 (EXPECTED)
```

Optional full local gate (fmt + clippy `-D warnings` + workspace tests,
~several minutes):

```bash
just gate-all
```

## 3. Install the binary on PATH

Desktop spawns `wayland-nano` by name (preset `cliCommand`), resolved via
PATH at spawn time:

```bash
sudo install -m 755 target/release/wayland-nano /usr/local/bin/wayland-nano
which wayland-nano && wayland-nano --version
```

Use `/usr/local/bin`, not a user-local dir: if Desktop is ever launched from
Finder/Dock (GUI PATH ≠ shell PATH), the binary must be in a system location.
When running Desktop dev from a terminal it inherits your shell PATH, but
`/usr/local/bin` is safe in both cases.

No Gatekeeper/notarization issue — locally built binaries run as-is.

## 4. Credentials

Nano draws on the providers connected in Wayland Desktop
(`authRequired: false` in the preset). For standalone CLI/TUI use:

- **Flux** (primary): `export FLUX_API_KEY=<key>` — or `FLUX_TEST_KEY`, or
  `FLUX_API_KEY_FILE=<path>` pointing at an owner-only (`chmod 600`) file.
- **Other providers** (OpenAI, Anthropic, xAI, OpenRouter, DeepSeek, …):
  the canonical env var per provider (or `<VAR>_FILE`), per the vendored
  catalog `crates/nano-model/data/providerCatalog.vendored.json`.

Never commit keys; never echo them into logs.

## 5. Containment on macOS (nothing to set up)

Nano's OS containment on macOS is **Seatbelt** (`sandbox-exec` with ported
`.sbpl` policies), CI-gated + runtime-gated on macOS 14 arm64. Unlike
Windows there is **no provisioning step** — no service accounts, no WFP.
If Seatbelt cannot be applied, Nano fails closed with
`SANDBOX_UNAVAILABLE`; there is no silent unsandboxed fallback.

## 6. Desktop: checkout + merge the error-table PR

```bash
git clone https://github.com/FerroxLabs/wayland.git
cd wayland
git checkout feature/wayland-nano
gh pr merge 954        # feat(acp): nano error table — RC2 surface (57 kinds)
```

PR #954 regenerates `src/common/types/nanoErrorCodes.ts` (+ JSON parity
copy) to the 57-kind RC2 error surface. It touches two generated files
only. Note: this branch line has pre-existing red CI from an unrelated
constitution-fs pin issue (PRs #950–953) — that is NOT caused by #954.

## 7. Run Desktop dev and pick Nano

```bash
bun install
bun run start          # electron-vite dev — launches the Electron app
```

In the UI: open the agent picker → **Wayland Nano** is always listed
(built-in, like Wayland Core — it shows even if the binary were missing).
Select it, start a session.

How it works under the hood (for debugging): Desktop's `AgentRegistry`
registers `wnano` (`src/common/types/acpTypes.ts` → `acpArgs: ['acp-host']`,
streaming on). At spawn, `AcpAgentManager` runs
`wayland-nano acp-host` and speaks ACP JSON-RPC over stdio:
initialize → session/new → session/prompt with `session/update` stream
chunks.

## 8. Smoke list (what "working" looks like)

1. `wayland-nano --version` works in a fresh terminal.
2. Picker lists Wayland Nano; selecting it opens a session without error.
3. A simple prompt streams a response (Flux key in env or a provider
   connected in Desktop).
4. A file-write request triggers a permission prompt; "Allow once" writes.
5. Cancel mid-turn leaves the session alive.
6. Quit + relaunch Desktop → the session resumes history
   (`session/load`).

If step 2/3 fails: confirm `which wayland-nano` **in the same terminal that
launched Desktop**, and check Desktop's dev console for spawn errors.

## 9. Standalone TUI (optional)

Nano has its own terminal UI (ratatui-based), like Codex/Claude Code. It is
a SEPARATE binary, not a subcommand:

```bash
cargo build --release -p nano-tui
sudo install -m 755 target/release/nano-tui /usr/local/bin/nano-tui
nano-tui
```

`wayland-nano` itself is the headless CLI: `doctor | protocol-host |
acp-host | auth login|status|logout <server> | exec | session fork |
sessions | goal | --version` (bare run prints usage, exit 2).

## 10. Known deferrals (not bugs — don't chase them)

- **Flux Auto routing on tool-bearing turns**: explicit `Auto` currently
  refuses pre-dispatch with `capability_empty` (tool-capability catalog not
  built yet; ladder is wired but unreachable on tool-bearing turns — all
  ACP turns). Pinned models and Flux aliases work fine. Known gap,
  design-compliant.
- **F-P2B-1**: the `view_image` tool is unreachable (vision-capable leaf
  not yet blessed in the catalog). Image INTAKE (attachments into a
  vision-capable pinned model) works; image-returning tools are the gap.
- **Review mode advertisement**: being flipped on in the in-flight P4 fix
  round (branch `feat/p4-fixround`); if your master predates that merge,
  `/review` works but the capability isn't advertised.
- **Not claimed**: macOS notarized distribution, Windows ARM64
  provisioning, non-Ubuntu Linux.

## 11. Where things are (wayland-nano repo)

- `docs/STATUS.md` — sprint state, known issues
- `docs/COMPATIBILITY.md` — per-platform support levels (what's proven vs
  claimed)
- `docs/FOLLOWUPS.md` — findings register (deferred debt)
- `docs/release/EVIDENCE-BUNDLE.md` — release evidence index
- `ARCHITECTURE.md` — system constitution; `UPSTREAM.md` — donor
  provenance ledger
- Error table source of truth: `crates/nano-session/src/error_codes.rs`
  (Desktop mirror is generated — never hand-edit either side)

## 12. Pending on the Windows side (not blockers for Mac bring-up)

- P4 fix round in flight (`feat/p4-fixround`): wires the exec-rules DSL
  (allow/prompt/deny rule auto-approval) into the gate + Windows ACL seam;
  flips review-mode advertisement on.
- P5 adversarial proof (Auto routing) — the last RC2 completion item.
