<#
.SYNOPSIS
    Build the `wayland-nano` release binary and stage it into the npm package layout.

.DESCRIPTION
    Cross-builds (or natively builds) crates/nano-cli's `wayland-nano` binary for one
    platform of the distribution matrix — or all of them — and copies the
    result into packaging/npm/binaries/<platform>-<arch>/, the layout that
    packaging/npm/bin/install.js and bin/wayland-nano.js resolve at install/runtime.

    win32-arm64 is compile-gate only (not supported at runtime); the launcher
    rejects it with a clear error. Cross targets require the matching rustup
    target and a working linker (e.g. Xcode CLT for darwin, a cross gcc for
    linux-arm64); building on the host platform always works.

.EXAMPLE
    pwsh packaging/npm/scripts/pack.ps1                      # host platform
    pwsh packaging/npm/scripts/pack.ps1 -Platform linux-x64
    pwsh packaging/npm/scripts/pack.ps1 -Platform all -SkipBuild
#>
[CmdletBinding()]
param(
    [ValidateSet('win32-x64', 'win32-arm64', 'darwin-x64', 'darwin-arm64', 'linux-x64', 'linux-arm64', 'all')]
    [string]$Platform = 'host',

    # Copy already-built binaries from target/<triple>/release without invoking cargo.
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..\..')
$PackageRoot = Resolve-Path (Join-Path $PSScriptRoot '..')

# npm platform-arch key -> Rust target triple. The key layout matches
# packaging/npm/bin/install.js exactly.
$Matrix = [ordered]@{
    'win32-x64'    = 'x86_64-pc-windows-msvc'
    'win32-arm64'  = 'aarch64-pc-windows-msvc'   # compile-gate only
    'darwin-x64'   = 'x86_64-apple-darwin'
    'darwin-arm64' = 'aarch64-apple-darwin'
    'linux-x64'    = 'x86_64-unknown-linux-gnu'
    'linux-arm64'  = 'aarch64-unknown-linux-gnu'
}

function Get-HostPlatformKey {
    if ($IsWindows) { return 'win32-x64' }
    if ($IsMacOS) {
        return ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') ? 'darwin-arm64' : 'darwin-x64'
    }
    return ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') ? 'linux-arm64' : 'linux-x64'
}

function Pack-One([string]$Key) {
    $triple = $Matrix[$Key]
    $exe = $Key.StartsWith('win32') ? 'wayland-nano.exe' : 'wayland-nano'
    $builtPath = Join-Path $RepoRoot "target\$triple\release\$exe"

    if (-not $SkipBuild) {
        Write-Host "==> cargo build --release -p nano-cli --target $triple"
        Push-Location $RepoRoot
        try {
            & cargo build --release -p nano-cli --target $triple
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed for $triple (exit $LASTEXITCODE)" }
        }
        finally {
            Pop-Location
        }
    }

    if (-not (Test-Path $builtPath)) {
        throw "expected binary not found: $builtPath (build first or drop -SkipBuild)"
    }

    $destDir = Join-Path $PackageRoot "binaries\$Key"
    New-Item -ItemType Directory -Force -Path $destDir | Out-Null
    Copy-Item -Force $builtPath (Join-Path $destDir $exe)
    Write-Host "==> staged $Key -> $destDir\$exe"
}

$targets = switch ($Platform) {
    'host' { @(Get-HostPlatformKey) }
    'all'  { @($Matrix.Keys) }
    default { @($Platform) }
}

foreach ($key in $targets) {
    Pack-One $key
}

Write-Host '==> done. Validate with: node --check packaging/npm/bin/install.js; node --check packaging/npm/bin/wayland-nano.js'
