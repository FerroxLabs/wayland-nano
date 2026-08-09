# C1.2 full-criterion proof — Track B

**Run AFTER live provisioning** (`scripts/provision/README.md`).
External-state oracle: every probe verifies real OS state (filesystem,
process list, network), never Nano's self-report.

## Run

```powershell
# From D:\Development\waylandnano\nano-k3 — standard user, NO WSL:
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\c12-proof\Test-C12Proof.ps1
```

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
| setup-idempotent | rerun makes no changes | marker hash equal |
| uninstall-scope | uninstall removes only Nano state | residue scan |

Probes requiring provisioned identities self-skip with `SKIP: not provisioned`
so the harness is also runnable pre-provisioning to validate the harness itself.
