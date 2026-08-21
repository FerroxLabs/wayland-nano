# NPM packaging acceptance v2 — win32-x64

Date: 2026-08-13. Host: Windows x86_64. Verdict: **PASS**.

Scratch artifacts were created under
`.tmp/p4-pkg-acceptance-v2-798dcf27786844e69d1f64d45e05b589/` and removed
after the run. No credential was read or used.

## Fresh build and staging

```text
cargo build --release -p nano-cli
Finished `release` profile [optimized]

pwsh -NoProfile -File packaging/npm/scripts/pack.ps1
==> cargo build --release -p nano-cli
==> staged win32-x64 (10994176 bytes)
==> wrote ...\packaging\npm\binaries-manifest.json
```

The generated manifest recorded SHA-256
`c40f0ddc2af6a2faed52522d184a54157819c23cd7e2310e9f90b9c4c7f38b7c`.
Both JavaScript files passed `node --check`.

## Exact dry-run payload

`npm pack --dry-run --json` exited 0 and reported six files, 4,544,299 bytes
packed and 11,001,254 bytes unpacked:

| Path | Bytes |
|---|---:|
| `README.md` | 2,233 |
| `bin/install.js` | 2,937 |
| `bin/wayland-nano.js` | 1,066 |
| `binaries-manifest.json` | 241 |
| `binaries/win32-x64/wayland-nano.exe` | 10,994,176 |
| `package.json` | 601 |

No source tree, environment file, key, or unrelated artifact was included.

## Clean-prefix lifecycle

The tarball was installed with registry access disabled:

```text
npm install --global --prefix <clean-prefix> --offline --no-audit --no-fund <tarball>
added 1 package in 885ms

<clean-prefix>\wayland-nano.cmd --version
wayland-nano 0.1.0

npm uninstall --global --prefix <clean-prefix> --offline --no-audit --no-fund waylandnano
removed 1 package in 395ms
```

After uninstall there were no package files or command shims. npm retained
only the empty `<clean-prefix>/node_modules/` directory.

## Fail-closed guards

An injected `freebsd-riscv64` resolution produced
`WaylandNanoPackagingError WAYLAND_NANO_UNSUPPORTED_PLATFORM` and listed the
five supported targets. A copied package whose binary had one byte appended
produced `WaylandNanoPackagingError WAYLAND_NANO_INTEGRITY_MISMATCH`.

## Result

| Check | Result |
|---|---|
| Fresh release build | PASS |
| Host staging + integrity manifest | PASS |
| JavaScript syntax | PASS |
| Exact npm dry-run payload | PASS |
| Offline clean-prefix install | PASS |
| Installed `--version` | PASS |
| Typed unsupported-platform refusal | PASS |
| Tamper refusal | PASS |
| Uninstall removes package and shims | PASS |
