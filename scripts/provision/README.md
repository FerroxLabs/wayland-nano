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
  Track A's `CodexSandbox*` accounts if those exist.
- The helper is idempotent: rerun makes no changes when state matches.
- Uninstall/teardown is NOT implemented yet — no uninstall mode exists in the
  setup helper. The proof's `uninstall-scope` probe audits that provisioning
  residue stays inside the known Nano-owned scope (accounts, group, the 3
  `codex_sandbox_offline_*` firewall rules, `%USERPROFILE%\.nanok3`).
- Windows Sandbox feature is NOT required for this path.

## After provisioning

Run the C1.2 full-criterion proof (`scripts/c12-proof/`): escape probes,
process-tree kill timing (≤5s), standard-user context checks, WFP deny
verification, and the evidence manifest for `shared/reviews/C1/`.
