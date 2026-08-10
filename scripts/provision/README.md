# Live provisioning — owner action package

**Status: READY, awaiting one elevated run by the owner.**
Nothing here runs without the owner's explicit go.

## What this does (once, elevated)

1. Creates local accounts `NanoK3SandboxOffline` and `NanoK3SandboxOnline`
   (DPAPI-protected passwords in `.sandbox-secrets`).
2. Installs WFP block-all filters for the offline identity (loopback proxy
   carve-out per settings).
3. Applies ACL grants/denies for the computed roots.
4. Writes the versioned setup marker (v5).

## The command

From an **elevated** PowerShell (right-click → Run as administrator), in
`D:\Development\waylandnano\nano-k3`:

```powershell
# 1. Review the payload that will be sent (read-only):
cargo run -p nano-sandbox --bin nanok3-provision-dry-run

# 2. Execute provisioning (this is the privileged step):
target\release\nanok3-sandbox-setup.exe <BASE64_PAYLOAD>
```

The payload is produced by calling `run_elevated_provisioning_setup` with
`WindowsSandboxProvisioningSettings { proxy_ports: [], allow_local_binding: false }`.
(An owner-reviewable dry-run bin will print the exact payload before anything
executes — B-PRV-01.)

## Safety notes

- Account names are Track-B namespaced (`NanoK3Sandbox*`) — no collision with
  Track A's `CodexSandbox*` accounts if those exist. The same holds for the
  `NanoK3SandboxUsers` group, `nanok3_sandbox_offline_*` firewall rules, and
  the `NanoK3 Windows Sandbox WFP` provider/filters (Track-B GUIDs).
- The helper is idempotent: rerun makes no changes when state matches.
- **Previously provisioned dev boxes:** before this rebrand, Track-B
  provisioning created Codex-branded state (group `CodexSandboxUsers`,
  `codex_sandbox_offline_*` firewall rules, WFP provider `Codex Windows
  Sandbox WFP` with donor GUIDs). The new uninstall mode only removes
  NanoK3-branded state, so any box provisioned before the rebrand should be
  cleaned of those legacy objects (delete the old group, firewall rules, and
  WFP provider/filters manually) before re-provisioning.
- Windows Sandbox feature is NOT required for this path.

## Uninstall (elevated)

The same helper removes ONLY the NanoK3-track machine state (the two
`NanoK3Sandbox*` accounts, the `NanoK3SandboxUsers` group when its membership
is exactly the provisioned accounts, the `nanok3_sandbox_*` firewall rules,
the NanoK3 WFP provider/sublayer/filters, and the setup marker):

```powershell
# elevated; payload built like the provisioning payload plus "uninstall": true
target\release\nanok3-sandbox-setup.exe <BASE64_UNINSTALL_PAYLOAD>
```

Uninstall is fail-closed: every removal is keyed by an exact NanoK3 name or a
Track-B WFP GUID verified before deletion, and it never touches Track A's
`CodexSandbox*` / `codex_*` objects. Afterwards, verify with
`scripts/c12-proof/Test-C12Proof.ps1 -PostUninstall`.

## After provisioning

Run the C1.2 full-criterion proof (`scripts/c12-proof/`): escape probes,
process-tree kill timing (≤5s), standard-user context checks, WFP deny
verification, and the evidence manifest for `shared/reviews/C1/`.
