# C1.2 full-criterion proof — Track B

**Run AFTER live provisioning** (`scripts/provision/README.md`).
External-state oracle: every probe verifies real OS state (filesystem,
process list, network), never Nano's self-report.

## Run

```powershell
# From D:\Development\waylandnano\nano-k3 — ELEVATED (admin) shell, NO WSL:
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\c12-proof\Test-C12Proof.ps1
```

**The authoritative run is elevated.** Four containment probes execute as the
provisioned OFFLINE IDENTITY (`NanoK3SandboxOffline`) via
`Start-Process -Credential` (CreateProcessWithLogonW), which is admin-gated on
this box — verified empirically: unelevated launch of even whoami.exe as the
offline account returns Access is denied. `write-outside-root`,
`sensitive-read-deny`, `junction-escape`, and `network-deny-offline` therefore
report `SKIP: requires elevation` unelevated — honest, not green.
(`netsh wfp` enumeration is also admin-only — and writes XML to a file, not
stdout — but survives only as a secondary detail line inside
`network-deny-offline`.) Unelevated runs remain useful pre-provisioning to
validate the harness itself.

Outputs `scripts/c12-proof/evidence/c12-manifest-<timestamp>.json` in the
BUILD_PLAN_V3 §8 manifest shape, then prints a PASS/FAIL summary line per probe.

## Probe matrix

| Probe | Criterion | Oracle |
|---|---|---|
| write-inside-root | workspace write allowed | file exists |
| write-outside-root | workspace-only writes | offline identity denied; file absent (child exit contract 42/43) |
| sensitive-read-deny | offline identity cannot read outside grants | offline identity read denied (child exit contract 44/45); controls: harness read of same file succeeds, same-user deny-ACL self-test |
| junction-escape | junction cannot escape deny | offline identity write through junction denied; no file outside root (child exit contract 46/47); control: same junction write succeeds for harness user |
| tree-kill | descendants dead ≤5s | PID-scoped CIM inventory: none of the probe's recorded descendant PIDs survive |
| process-cleanup | no orphan helpers | CIM process scan |
| network-deny (offline identity) | offline identity TCP connect fails | real TCP connect to api.fluxrouter.ai:443 as offline identity fails (child exit contract 48/49); control: same connect as harness user succeeds |
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
- **Offline-identity probes** (`write-outside-root`, `sensitive-read-deny`,
  `junction-escape`, `network-deny-offline`) all execute their attempt as the
  provisioned `NanoK3SandboxOffline` identity, never as the harness user.
  They share one pattern (`Get-OfflineCredential` / `Invoke-OfflineChild`):
  the offline credential is decrypted from
  `%USERPROFILE%\.nanok3\.sandbox-secrets\sandbox_users.json` (DPAPI,
  current-user scope) **in memory only** — the plaintext is never printed,
  logged, or persisted — and a one-shot powershell child is launched via
  `Start-Process -Credential` under a numeric exit-code contract:
  42/43 write, 44/45 read, 46/47 junction write, 48/49 TCP connect (even =
  violation, odd = denied/blocked, expected). stdout/stderr are captured to
  `nanok3-<probe>.{stdout,stderr}.log` in the evidence dir as the denial
  evidence. Each probe also carries a harness-side control so the denial is
  attributable to the identity: `sensitive-read-deny` requires the harness
  user to read the same file successfully (plus the legacy same-user
  deny-ACL self-test); `junction-escape` requires the harness user's write
  through the same junction to land; `network-deny-offline` requires the
  harness user's connect to `api.fluxrouter.ai:443` to succeed. A failed
  control makes the probe FAIL as inconclusive rather than PASS.
- **network-deny-offline**'s primary oracle is the real TCP connect attempt
  as the offline identity. The pre-hardening WFP-XML grep (`netsh wfp show
  filters`, NanoK3 substring) is kept only as a secondary `wfp-filters=`
  detail line — filter presence alone never passes the probe.
- **tree-kill-5s** scopes its survivor inventory to the probe's own tree:
  while `nanok3-tree-kill-probe.exe` runs, the harness walks
  `Win32_Process.ParentProcessId` from the probe PID every 100 ms and records
  every descendant PID observed; after `job.terminate()` none of those
  recorded PIDs may still exist. An empty recording FAILs the probe (the
  inventory would be vacuous). The earlier host-wide `ping.exe` scan could
  misattribute unrelated processes (panel finding #5) and is gone.
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
