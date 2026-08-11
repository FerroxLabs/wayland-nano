# @waylandnano/nano (npm package)

Alpha npm packaging for the Wayland Nano CLI. The package ships prebuilt
native binaries and a zero-dependency Node shim — no compilation at install
time, no third-party npm dependencies.

> Alpha: `private: true`, unsigned. Do not publish to the public registry yet.

## Install flow (alpha)

From the repo, on the machine/CI lane matching your target platform:

```powershell
# Build the release binary and stage it into this package
pwsh packaging/npm/scripts/pack.ps1                 # host platform
pwsh packaging/npm/scripts/pack.ps1 -Platform all   # full matrix (needs cross toolchains)
```

Then pack and install the tarball:

```sh
cd packaging/npm
npm pack                          # produces waylandnano-nano-0.1.0-alpha.0.tgz
npm install -g ./waylandnano-nano-0.1.0-alpha.0.tgz
wayland-nano --help
```

`postinstall` (`bin/install.js`) selects the binary for your platform/arch
from `binaries/<platform>-<arch>/` and fails the install if none is staged.
The `wayland-nano` command on your PATH is `bin/wayland-nano.js`, a shim that
execs the native binary with your arguments and forwards its exit code and
signals.

Supported at runtime: `win32-x64`, `darwin-x64`, `darwin-arm64`, `linux-x64`,
`linux-arm64`. `win32-arm64` is compile-gated only — the binary is built to
keep the target compiling, but the launcher rejects it at runtime with a
clear error.

## Layout

```
packaging/npm/
├── package.json          # private alpha manifest, bin + postinstall wiring
├── bin/
│   ├── install.js        # postinstall: resolve + chmod the platform binary
│   └── wayland-nano.js   # PATH shim: exec binary, argv/exit/signal passthrough
├── binaries/             # staged by scripts/pack.ps1 (not committed)
│   └── <platform>-<arch>/wayland-nano[.exe]
└── scripts/
    └── pack.ps1          # cargo build --release + stage into binaries/
```
