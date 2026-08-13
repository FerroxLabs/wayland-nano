# Wayland Nano — Mac build & Desktop integration handoff

Audience: the macOS build machine. Goal: build `wayland-nano` from source,
run it standalone, and integrate it into Wayland Desktop as the first-class
`wnano` agent.

State of the code: master @ `71ba0e7` (post P3 fix-round), CI 6/6 green.
All RC2 packs merged: P1 (web search + cost metering), P2a (image intake),
P2b (image tool results), P3 (MCP full-duplex + ToolSearch + resources +
elicitation + OAuth PKCE), P4 (exec rules DSL, review mode, persistent PTY,
repomap, session browser), P5 (Flux Auto routing).

---

## 1. Prereqs

- macOS 14+ (arm64 verified in CI; x86_64 also CI-gated on macOS 15)
- Rust via rustup — the repo pins **1.95.0** in `rust-toolchain.toml`;
  rustup auto-selects it on first cargo invocation
- Xcode CLT (`xcode-select --install`)
- `just` (`brew install just`) for the gate; optional for building
- Bun for the Desktop repo (`brew install oven-sh/bun/bun` or per Desktop README)

## 2. Clone and build

```bash
git clone https://github.com/FerroxLabs/wayland-nano.git
cd wayland-nano
cargo build --release
```

Binary: `target/release/wayland-nano`. Smoke checks:

```bash
./target/release/wayland-nano --version
./target/release/wayland-nano            # bare run prints usage, exit 2 (expected)
```

Full local gate (fmt + clippy -D warnings + workspace tests), optional but
recommended once:

```bash
just gate-all
```

## 3. Put it on PATH

Desktop spawns `wayland-nano` via the `wnano` preset with
`cliCommand: 'wayland-nano'`, `acpArgs: ['acp-host']` — it must resolve on
PATH:

```bash
sudo install -m 755 target/release/wayland-nano /usr/local/bin/wayland-nano
# or: ln -s "$(pwd)/target/release/wayland-nano" /usr/local/bin/wayland-nano
```

Verify: `which wayland-nano && wayland-nano --version`.

No Gatekeeper/notarization issue — locally built binaries run fine.

## 4. Credentials

Nano draws on the providers connected in Wayland Desktop
(`authRequired: false` in the preset). For standalone/CLI use:

- Flux: `export FLUX_API_KEY=<key>` (or `FLUX_TEST_KEY`, or
  `FLUX_API_KEY_FILE=<path to owner-only file>`). Resolution order is
  `crates/nano-cli/src/flux_key.rs`.
- Other catalog providers: canonical env var per provider (or `<VAR>_FILE`),
  per `crates/nano-cli/src/provider_key.rs` and the vendored catalog at
  `crates/nano-model/data/providerCatalog.vendored.json`.

Never commit keys. Key files must be `chmod 600`.

## 5. Containment on macOS

Seatbelt (`sandbox-exec`) backend with ported `.sbpl` policies —
CI-gated + runtime-gated on macOS 14 arm64. No provisioning step is needed
on macOS (that's the Windows `NanoSandbox*` account machinery). If the
sandbox cannot be applied, Nano fails closed with `SANDBOX_UNAVAILABLE` —
there is no silent unsandboxed fallback.

## 6. Desktop integration

```bash
git clone https://github.com/FerroxLabs/wayland.git
cd wayland
git checkout feature/wayland-nano
```

Then merge the error-table PR (open at time of writing):

- PR #954 `feat/nano-error-table-rc2` → `feature/wayland-nano`:
  regenerates `src/common/types/nanoErrorCodes.ts` (+ JSON parity copy) to
  the 57-kind RC2 surface. Merge via GitHub or `gh pr merge 954`.

The `wnano` preset is already first-class on this branch
(`src/common/types/acpTypes.ts`): listed in the agent registry, streaming
enabled, spawns `wayland-nano acp-host` over stdio. No config-writing needed.

Run Desktop dev:

```bash
bun install
bun run dev   # or the repo's documented dev command
```

Pick **Wayland Nano** in the agent picker. First turn exercises:
initialize → session/new → streamed response. If the picker errors, check
`which wayland-nano` in the same shell Desktop was launched from (GUI apps
on macOS have a different PATH — if launched from Finder/Dock, symlink into
`/usr/local/bin` specifically, not a user-local dir, or launch Desktop from
the terminal).

## 7. What to verify (smoke list)

1. `wayland-nano --version` in a fresh terminal
2. Desktop agent picker lists Wayland Nano; selecting it starts a session
3. A simple prompt streams a response ( Flux key present in env or Desktop
   provider config)
4. A file-write request triggers a permission prompt; Allow once writes
5. Cancel mid-turn leaves the session alive
6. Quit + relaunch Desktop → session/load resumes history

## 8. Known deferrals (not bugs)

- **Auto routing on tool-bearing turns**: production Auto turns currently
  refuse pre-dispatch with `capability_empty` (the tool-capability catalog
  doesn't exist yet — the ladder is wired but unreachable on tool-bearing
  turns, i.e. all ACP/exec turns). Pinned/alias models work; explicit Auto
  is proven at the seam. Design-§3-compliant; a known gap, not a regression.
- **F-P2B-1**: `view_image` tool unreachable (vision_backed conjuncts
  mutually unsatisfiable). Unblock = bless an `anthropic:` catalog id after
  live probe, or wire `FluxDriver::anthropic_compat`. Post-merge wave item.
- **Review-mode advertisement**: `nanoExtensions` review capability is
  pinned OFF until the P4 proof's §14 leg-2 live run flips it.
- macOS notarized distribution, Windows ARM64 provisioning: not claimed.

## 9. Pending on the Windows side (not blockers for Mac bring-up)

- P3 leg-6 OAuth live re-proof, P4 adversarial proof, P5 adversarial proof
  (briefs in `.tmp/p4-proof/ASSIGNMENT.md`, `.tmp/p5-proof/ASSIGNMENT.md`)
- Desktop repo has pre-existing red CI from a constitution-fs pin issue on
  PRs #950-953 — unrelated to Nano; owner-side fix.

## 10. If something breaks

- `docs/STATUS.md` — sprint state and known issues
- `docs/COMPATIBILITY.md` — what is claimed vs not claimed per platform
- `docs/FOLLOWUPS.md` — filed findings register
- `docs/release/EVIDENCE-BUNDLE.md` — release evidence index
