# NPM packaging acceptance — G-PKG-1

Date: 2026-08-10. Host: Windows 11 x86_64, Git Bash + PowerShell 7.
Toolchain: node v24.16.0, npm 11.13.0, cargo 1.95.0 (pinned).
Verdict: **PASS** — all steps green, zero bugs found in the scaffold.

All scratch artifacts live under `waylandnano/.tmp/g-pkg-1/` (tarball, clean
prefix, captured outputs) — nothing was left in the repo. `.gitignore` covers
`packaging/npm/binaries/` and (added during this acceptance) `packaging/npm/*.tgz`.

## 1. Rebuild + stage

```
cd nano-k3
cargo build --release -p nano-cli          # Finished release profile, 0 warnings
pwsh -NoProfile -File packaging/npm/scripts/pack.ps1
# ==> cargo build --release -p nano-cli --target x86_64-pc-windows-msvc
# ==> staged win32-x64 -> ...\packaging\npm\binaries\win32-x64\nanok3.exe
# ==> done.
```

`node --check bin/install.js` and `node --check bin/nanok3.js` both pass.

## 2. npm pack (into .tmp, not the repo)

```
cd packaging/npm
npm pack --pack-destination /d/Development/waylandnano/.tmp/g-pkg-1/tarball
```

Tarball `nanok3-0.1.0-alpha.0.tgz` (2.8 MB packed / 6.4 MB unpacked), exactly
5 files: `package.json`, `README.md`, `bin/install.js`, `bin/nanok3.js`,
`binaries/win32-x64/nanok3.exe`. No stray files leaked into the package.

## 3. Offline install into a clean prefix

```
npm install -g --prefix .tmp/g-pkg-1/prefix --offline --no-audit --no-fund \
  .tmp/g-pkg-1/tarball/nanok3-0.1.0-alpha.0.tgz
# added 1 package in 722ms   (exit 0)
```

`--offline` makes npm hard-fail (ENOTCACHED) on any registry fetch, so exit 0
is proof the install touched **no network**: zero-dependency claim verified.
Prefix got `nanok3`, `nanok3.cmd`, `nanok3.ps1` shims + `node_modules/nanok3/`.
Postinstall ran (verified separately: `node prefix/node_modules/nanok3/bin/install.js`
prints `nanok3: using prebuilt binary for win32-x64`, exit 0).

## 4. Installed launcher — doctor + acp-host

```
export FLUX_API_KEY_FILE='D:\Development\waylandnano\.secrets\flux-test-key'  # path only
.tmp/g-pkg-1/prefix/nanok3 doctor
```

```
nanok3 doctor — 0.1.0
  PASS  os                       windows / x86_64
  PASS  shell-cmd                cmd.exe executes natively
  PASS  shell-powershell         powershell.exe executes natively
  WARN  sandbox-provisioning     not provisioned — elevated setup required ...
  PASS  egress-policy            non-allowlisted hosts denied at construction
  PASS  journal                  append + read-back verified
  PASS  sensitive-file-policy    .env denied; notes.txt allowed
  PASS  process-hygiene          no stray nanok3-* helper processes
  PASS  flux-credential          Flux credential resolvable (env or FLUX_API_KEY_FILE)
summary: 0 fail, 1 warn        → exit 0
```

(The sandbox-provisioning WARN is by design: unprovisioned is warn-not-fail
until elevated setup runs.)

```
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true}}}}' \
  | .tmp/g-pkg-1/prefix/nanok3 acp-host        # exit 0, stderr empty
```

Response frame (asserted `id==1`, `agentInfo.name=="nanok3"`, `protocolVersion==1`):

```json
{"jsonrpc":"2.0","id":1,"result":{"agentCapabilities":{"loadSession":false,"promptCapabilities":{"embeddedContext":false,"image":false,"text":true}},"agentInfo":{"name":"nanok3","version":"0.1.0"},"protocolVersion":1}}
```

Credential canary: a Node check compared the key-file value against every
captured output (acp-response, acp stderr, doctor output) in-memory — absent
from all. The key value was never printed or written to any artifact.

## 5. Negative checks

The installer/launcher resolve `process.platform`/`process.arch` directly and
expose no env override, so the guards were runtime-tested by redefining those
properties before requiring the scripts (plus code review):

```
node -e "Object.defineProperty(process,'platform',{value:'freebsd'});
         Object.defineProperty(process,'arch',{value:'riscv64'});
         require('./bin/install.js')"
# nanok3: no prebuilt binary for freebsd-riscv64.
#   This alpha ships binaries for: win32-x64          → exit 1 (install fails loudly)

node -e "Object.defineProperty(process,'platform',{value:'win32'});
         Object.defineProperty(process,'arch',{value:'arm64'});
         require('./bin/nanok3.js')" -- doctor
# nanok3: win32-arm64 is not supported at runtime in this alpha.
#   The ARM64 Windows build is compile-gated only. Use win32-x64, darwin-x64, ...  → exit 1
```

Code review confirms the launcher checks `UNSUPPORTED_RUNTIME` before the
binary-exists check, so compile-gated `win32-arm64` gets the specific message
even when a binary is staged; unknown platforms get the "no prebuilt binary"
error listing what the tarball ships. Neither path can proceed silently.

## Summary

| Step                                   | Result |
|----------------------------------------|--------|
| 1. rebuild + pack.ps1 staging          | PASS   |
| 2. npm pack (5 files, clean contents)  | PASS   |
| 3. offline install, clean prefix       | PASS   |
| 4a. `nanok3 doctor` exit 0             | PASS   |
| 4b. `acp-host` initialize handshake    | PASS   |
| 5. platform guards refuse loudly       | PASS   |

Bugs found: none in the scaffold. One repo-hygiene gap fixed: `npm pack`
tarballs were not gitignored — added `/packaging/npm/*.tgz` to
`nano-k3/.gitignore`.
