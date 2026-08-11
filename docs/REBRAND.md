# Rebrand work order — NanoK3 → Wayland Nano

Track B is the master implementation and takes the product name. This file is
the authoritative rename map. Execute as ONE atomic sweep (swarm), full gate
after, then the provisioned-state migration (elevated owner step).

## Naming decisions (final)

| Old | New |
|---|---|
| `nanok3.exe` / `nanok3-*` helper bins | `wayland-nano.exe` / `wayland-nano-*` |
| workspace root package `nanok3` | `wayland-nano` (crates stay `nano-*`) |
| `NanoK3Sandbox{Offline,Online}` | `NanoSandbox{Offline,Online}` (SAM 20-char cap) |
| `NanoK3SandboxUsers` group | `NanoSandboxUsers` |
| `nanok3_sandbox_*` firewall rules | `nano_sandbox_*` |
| `NanoK3 Windows Sandbox WFP` + GUIDs | `Wayland Nano Sandbox WFP` + fresh GUIDs |
| `~/.nanok3` home | `~/.nano` (override env `NANO_HOME`) |
| `NANOK3_*` env keys | `NANO_*` |
| `nanok3-linux-sandbox` helper | `wayland-nano-linux-sandbox` |
| npm package | `waylandnano` (unscoped — parity with `getwayland`) |
| ACP agentInfo.name / Desktop agent id | `wayland-nano` / display "Wayland Nano" |
| git identity in repo | "Wayland Nano" |

## In scope (rename)

- Root `Cargo.toml`, all crate `[[bin]]` names, `nano-cli` binary modes/messages.
- `crates/nano-sandbox`: account/group/firewall/WFP/marker/secrets paths,
  payload fields, UserList registry values, log dir names, test fixtures.
- `packaging/npm/` (name, install.js platform map, launcher, pack.ps1, README).
- `scripts/` (c12-proof, provision, canary, flux-probe) incl. NanoK3* test
  artifact namespacing.
- `.github/workflows/gate.yml` (helper binary names, artifact names).
- `README.md`, `AGENTS.md`, `docs/` (EXCEPT historical evidence — below).
- `.gitattributes`/`.gitignore` path references that mention old names.
- Both Desktop profile configs (`acp.customAgents` + `assistants` records):
  new id `wayland-nano`, new binary path, display name.

## Out of scope (do NOT touch — historical evidence)

- `scripts/c12-proof/evidence/*` manifests (they record what happened under
  the old name; renaming history falsifies it).
- `shared/reviews/**` verdicts, claims, panel files (same rule).
- `docs/metrics/*.md` historical numbers (append new-name numbers instead).
- `UPSTREAM.md` rows stay as written (they describe the port at that time).

## Migration (elevated, owner-run, after the sweep)

Staged kit (in `../.tmp/`): `nanok3-sandbox-setup.exe` (pre-rebrand helper,
kept because the rebranded helper's fail-closed guards REFUSE to touch
NanoK3* identities), `nanok3-uninstall-payload.txt`,
`wayland-nano-provision-payload.txt` (regenerate via the dry-run bin if the
schema moved).

1. Uninstall NanoK3-branded state with the PRE-REBRAND helper:
   `.tmp\nanok3-sandbox-setup.exe (Get-Content .tmp\nanok3-uninstall-payload.txt)`
2. Verify old state is gone directly:
   `Get-LocalUser NanoK3SandboxOffline` → not found; `Test-Path ~\.nanok3` → False.
3. Provision with the renamed helper:
   `target\release\wayland-nano-sandbox-setup.exe (Get-Content ..\.tmp\wayland-nano-provision-payload.txt)`
4. Full harness run → 12/12 under the new names.
5. Desktop: re-register agent (new id/path), one smoke conversation.

## Gate after sweep

`cargo test --workspace` + `cargo clippy --workspace --all-targets -- -D warnings`
+ `cargo fmt --all -- --check` all green; CI 6/6 on the push; `just gate-all`.
