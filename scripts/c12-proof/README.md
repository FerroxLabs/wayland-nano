# C1.2 full-criterion proof — Track B

**Run AFTER live provisioning** (`scripts/provision/README.md`).
External-state oracle: every probe verifies real OS state (filesystem,
process list, network), never Nano's self-report.

## Run

```powershell
# From D:\Development\waylandnano\nano-k3 — ELEVATED (admin) shell, NO WSL:
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\c12-proof\Test-C12Proof.ps1
```

**The authoritative run is elevated.** Two probes are admin-gated by the OS,
not by us: `network-deny-offline` (netsh WFP enumeration is admin-only — and
it writes XML to a file, not stdout) and `write-outside-root`
(`Start-Process -Credential` / CreateProcessWithLogonW is admin-gated on this
box). Unelevated they report `SKIP: requires elevation` — honest, not green.
Unelevated runs remain useful pre-provisioning to validate the harness itself.

Outputs `scripts/c12-proof/evidence/c12-manifest-<timestamp>.json` in the
BUILD_PLAN_V3 §8 manifest shape, then prints a PASS/FAIL summary line per probe.

## Probe matrix

| Probe | Criterion | Oracle |
|---|---|---|
| write-inside-root | workspace write allowed | file exists |
| write-outside-root | workspace-only writes | file absent |
| sensitive-read-deny | denied read fails | read throws |
| junction-escape | junction cannot escape deny | file absent |
| tree-kill | descendants dead ≤5s | CIM process scan |
| process-cleanup | no orphan helpers | CIM process scan |
| network-deny (offline identity) | TCP connect fails | socket error |
| broker-network-ok | broker reaches Flux | HTTPS 200 |
| path-edgecases | long path, Unicode, reserved names | create/read |
| setup-idempotent | marker refresh round-trips; tamper restored | canonical marker hash + readiness rule |
| uninstall-scope | provisioning residue in scope / uninstall removes only Nano state | residue scan (two modes, see below) |

Probes requiring provisioned identities self-skip with `SKIP: not provisioned`
so the harness is also runnable pre-provisioning to validate the harness itself.

## Uninstall-scope probe modes

Default (run while provisioned): asserts every Nano-owned artifact stays
inside the known scope — `NanoK3Sandbox{Offline,Online}` accounts,
`NanoK3SandboxUsers` group, the `nanok3_sandbox_offline_*` firewall rules,
the `NanoK3 Windows Sandbox WFP` provider + `nanok3_wfp_*` filters, and
`%USERPROFILE%\.nanok3` — with no services, scheduled tasks, or stray
profile-root files.

Post-uninstall (run after teardown): asserts every one of those artifacts is
gone — including the DPAPI secrets file
(`%USERPROFILE%\.nanok3\.sandbox-secrets\sandbox_users.json`), the
`%USERPROFILE%\.nanok3\.sandbox` log dir, and the
`HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList`
values that hid the sandbox accounts from the login screen. Teardown is the
setup helper itself with an `uninstall: true` payload (same base64 CLI
contract as provisioning), run elevated:

```powershell
# elevated; payload built like the provisioning payload plus "uninstall": true
target\release\nanok3-sandbox-setup.exe <BASE64_UNINSTALL_PAYLOAD>

# then, unelevated:
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\c12-proof\Test-C12Proof.ps1 -PostUninstall
```

The helper's uninstall is fail-closed on exact NanoK3 identities (account
name prefix, exact group membership, exact firewall rule names, Track-B WFP
GUIDs whose display names are verified before deletion, a secrets file that
is parsed and verified to name exactly the provisioned `NanoK3Sandbox*`
accounts before deletion, UserList values keyed by exact `NanoK3Sandbox*`
account names, and a log-dir removal guarded to the `.nanok3`-scoped
`.sandbox` path) and removes nothing else — Track A's
`CodexSandbox*`/`codex_*` objects are never touched.

## Known limitations / platform notes

- **Reserved DOS device names are writable on Windows 11 builds ≥ 26100.**
  Recorded anomaly: manifest `c12-manifest-20260809T233109Z.json` showed
  `path-edgecases PASS` with `reserved-name:WROTE(unexpected)`. Investigation
  (2026-08-10, build 26200, unelevated): `Out-File` created `aux`, `aux.txt`,
  `con`, `con.txt`, `nul.txt`, `com1.txt`, `clock$.txt` as ordinary NTFS files;
  the legacy Win32 reserved-name rejection no longer fires on create (note:
  `Test-Path`/`Get-Item` on such a name still returns not-found — only
  directory enumeration sees the file). This is **not a containment hole**:
  nano-sandbox enforcement is token+DACL based (`crates/nano-sandbox/src/token.rs`
  WRITE_RESTRICTED restricted token + capability SIDs, `acl.rs` allow/deny ACEs)
  and attaches to the resolved NT object regardless of Win32 name spelling —
  nothing in the crate parses or special-cases reserved names. A reserved-name
  file inside the workspace root is as contained as any other file; the same
  name outside the root is denied like any other path. The probe's original
  "should be rejected" expectation was wrong for this platform and has been
  relabeled accordingly.
- **write-outside-root** decrypts the offline account credential from
  `%USERPROFILE%\.nanok3\.sandbox-secrets\sandbox_users.json` (DPAPI,
  current-user scope) in memory only and launches a one-shot child as
  `NanoK3SandboxOffline` that attempts a write in `$WorkspaceRoot\outside`.
  Child exit 43 = denied (expected), 42 = wrote (FAIL). stdout/stderr are
  captured to `nanok3-write-outside-root.{stdout,stderr}.log` in the evidence
  dir as the denial evidence.
- **uninstall-scope** has two modes (see "Uninstall-scope probe modes"
  above). The default mode audits provisioning residue; `-PostUninstall`
  audits that the helper's `uninstall: true` run removed all NanoK3 machine
  state: accounts, group, firewall rules, WFP provider/filters, setup marker,
  the DPAPI secrets file, the `.sandbox` log dir, and the Winlogon
  `SpecialAccounts\UserList` hide-values. Provisioning artifacts are Track-B
  namespaced: group `NanoK3SandboxUsers`, firewall rules
  `nanok3_sandbox_offline_*`, WFP provider `NanoK3 Windows Sandbox WFP` with
  `nanok3_wfp_*` filters (Track-B GUIDs, distinct from the donor's).
- `setup-idempotent` builds a base64 setup payload with
  `refresh_marker_only: true` (schema field in
  `crates/nano-sandbox/src/bin/setup_main/win.rs`) and invokes
  `nanok3-sandbox-setup.exe <payload>` unelevated. The helper then performs
  ONLY the marker path provisioning runs (prepare the protected marker file,
  then commit valid contents). The probe verifies (a) the refresh round-trips:
  the marker still satisfies the readiness rule from
  `crates/nano-sandbox/src/identity.rs` (`load_marker`: parseable
  `SetupMarker` + matching `version`) and a canonical SHA-256 over every field
  except `created_at` — which `commit_setup_marker` legitimately re-stamps on
  each refresh — is unchanged; and (b) a tampered marker (version zeroed) is
  detected by that same readiness rule and restored to the pre-refresh content
  by a second refresh. A helper that no-ops fails the tamper half.
