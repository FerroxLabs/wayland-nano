# Live provisioning — owner action package

**Status: READY, awaiting one elevated run by the owner.**
Nothing here runs without the owner's explicit go.

> **Rebrand migration (NanoK3 → Wayland Nano):** the identities below are the
> NEW names (`NanoSandbox*`, `nano_sandbox_*`, `Wayland Nano Sandbox WFP`,
> `~/.nano`). Boxes provisioned under the old NanoK3 names must be migrated —
> uninstall with the old helper, verify zero residue, re-provision with the
> renamed helper — exactly as specified in `docs/REBRAND.md` ("Migration"
> section). Do not provision the new names over a live NanoK3 install.

## What this does (once, elevated)

1. Creates local accounts `NanoSandboxOffline` and `NanoSandboxOnline`
   (DPAPI-protected passwords in `.sandbox-secrets`).
2. Installs WFP block-all filters for the offline identity (loopback proxy
   carve-out per settings).
3. Applies ACL grants/denies for the computed roots.
4. Writes the versioned setup marker (v5).

## The command

From an **elevated** PowerShell (right-click → Run as administrator), in
`D:\Development\waylandnano\wayland-nano`:

```powershell
# 1. Review the payload that will be sent (read-only):
cargo run -p nano-sandbox --bin wayland-nano-provision-dry-run

# 2. Execute provisioning (this is the privileged step):
target\release\wayland-nano-sandbox-setup.exe <BASE64_PAYLOAD>
```

The payload is produced by calling `run_elevated_provisioning_setup` with
`WindowsSandboxProvisioningSettings { proxy_ports: [], allow_local_binding: false }`.
(An owner-reviewable dry-run bin will print the exact payload before anything
executes — B-PRV-01.)

## Safety notes

- Account names are namespaced (`NanoSandbox*`) — no collision with
  Track A's `CodexSandbox*` accounts if those exist. The same holds for the
  `NanoSandboxUsers` group, `nano_sandbox_offline_*` firewall rules, and
  the `Wayland Nano Sandbox WFP` provider/filters (fresh GUIDs).
- The helper is idempotent: rerun makes no changes when state matches.
- **Previously provisioned dev boxes:** before the original Track-B rebrand,
  provisioning created Codex-branded state (group `CodexSandboxUsers`,
  `codex_sandbox_offline_*` firewall rules, WFP provider `Codex Windows
  Sandbox WFP` with donor GUIDs); the NanoK3 → Wayland Nano rebrand then
  renamed the Track-B identities (see `docs/REBRAND.md`). The uninstall mode
  only removes Wayland Nano–branded state, so any box carrying legacy
  Codex-branded or NanoK3-branded objects should be cleaned of those (delete
  the old groups, firewall rules, and WFP providers/filters manually, or per
  the `docs/REBRAND.md` migration for NanoK3 state) before re-provisioning.
- Windows Sandbox feature is NOT required for this path.

## Uninstall (elevated)

The same helper removes ONLY the Wayland Nano machine state (the two
`NanoSandbox*` accounts, the `NanoSandboxUsers` group when its membership
is exactly the provisioned accounts, the `nano_sandbox_*` firewall rules,
the Wayland Nano WFP provider/sublayer/filters, the Winlogon
`SpecialAccounts\UserList` values hiding the sandbox accounts, the DPAPI
credential file `.sandbox-secrets\sandbox_users.json`, the setup marker, and
the `.sandbox` log dir):

```powershell
# elevated; payload built like the provisioning payload plus "uninstall": true
target\release\wayland-nano-sandbox-setup.exe <BASE64_UNINSTALL_PAYLOAD>
```

Uninstall is fail-closed: every removal is keyed by an exact Wayland Nano name
or a WFP GUID verified before deletion, and it never touches Track A's
`CodexSandbox*` / `codex_*` objects. The secrets file is deleted only after
being parsed and verified to name exactly the provisioned `NanoSandbox*`
accounts (a mismatch aborts the run with the file left in place), the
UserList values are keyed by the exact `NanoSandbox*` account names, and the
`.sandbox` log-dir removal is guarded to the `.nano`-scoped path. Afterwards,
verify with `scripts/c12-proof/Test-C12Proof.ps1 -PostUninstall`.

## After provisioning

Run the C1.2 full-criterion proof (`scripts/c12-proof/`): escape probes,
process-tree kill timing (≤5s), standard-user context checks, WFP deny
verification, and the evidence manifest for `shared/reviews/C1/`.
