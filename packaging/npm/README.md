# waylandnano

Wayland Nano is a zero-dependency npm wrapper around prebuilt native CLI
binaries. Installation performs no compilation and verifies the selected
binary's byte size and SHA-256 digest before making it executable.

## Install, run, uninstall

```sh
npm install --global waylandnano@next
wayland-nano --version
npm uninstall --global waylandnano
```

The alpha channel uses npm's `next` dist-tag. Stable releases use `latest`.
Node.js 18 or newer is required.

Supported platforms are Windows x64, macOS x64/arm64, and Linux x64/arm64.
Other platform/architecture pairs fail installation with the typed
`WAYLAND_NANO_UNSUPPORTED_PLATFORM` diagnostic. A missing or altered binary
fails closed with an integrity diagnostic; there is no source-build fallback.

## Release staging

Stage a freshly built native host binary from `target/release`:

```powershell
pwsh packaging/npm/scripts/pack.ps1
```

An explicit cross target is read from `target/<rust-triple>/release`:

```powershell
pwsh packaging/npm/scripts/pack.ps1 -Platform linux-x64 -SkipBuild
```

The release workflow downloads each native build artifact into this layout:

```text
artifacts/npm-binaries/
├── win32-x64/wayland-nano.exe
├── darwin-arm64/wayland-nano
├── darwin-x64/wayland-nano
├── linux-x64/wayland-nano
└── linux-arm64/wayland-nano
```

It then assembles the complete package without cross-compiling:

```powershell
pwsh packaging/npm/scripts/pack.ps1 -Platform all -ArtifactRoot artifacts/npm-binaries
```

`pack.ps1` writes `binaries-manifest.json` with each platform binary's exact
filename, size, and SHA-256 digest. The staged `binaries/` tree is ignored by
git and must never be hand-committed.

Publishing is restricted to explicit `v*` tags in `.github/workflows/release.yml`.
The owner supplies `NPM_TOKEN` as a repository secret at publish time; it is
never stored in this package or printed by the workflow.

## Package-name readiness

The intended public name is `waylandnano`. Registry ownership/availability is
external state and must be confirmed by the owner immediately before the first
publish. The workflow does not claim or reserve the name and performs no
publish on branch pushes or merges.
