<#
.SYNOPSIS
    Stage Wayland Nano binaries and write the npm integrity manifest.
.DESCRIPTION
    With no ArtifactRoot, builds/stages the host from target/release. Explicit
    cross targets use target/<rust-triple>/release. With ArtifactRoot, expects
    <ArtifactRoot>/<platform-arch>/wayland-nano[.exe], the merged layout emitted
    by the release workflow, and stages every requested target without building.
.EXAMPLE
    pwsh packaging/npm/scripts/pack.ps1
    pwsh packaging/npm/scripts/pack.ps1 -Platform linux-x64
    pwsh packaging/npm/scripts/pack.ps1 -Platform all -ArtifactRoot artifacts/npm-binaries
#>
[CmdletBinding()]
param(
    [ValidateSet('host', 'win32-x64', 'darwin-x64', 'darwin-arm64', 'linux-x64', 'linux-arm64', 'all')]
    [string]$Platform = 'host',
    [string]$ArtifactRoot,
    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$PackageRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Matrix = [ordered]@{
    'win32-x64'    = 'x86_64-pc-windows-msvc'
    'darwin-arm64' = 'aarch64-apple-darwin'
    'darwin-x64'   = 'x86_64-apple-darwin'
    'linux-x64'    = 'x86_64-unknown-linux-gnu'
    'linux-arm64'  = 'aarch64-unknown-linux-gnu'
}

function Get-HostPlatformKey {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ([System.Environment]::OSVersion.Platform -eq 'Win32NT') {
        if ($arch -ne 'X64') { throw "unsupported build host architecture: win32-$($arch.ToString().ToLowerInvariant())" }
        return 'win32-x64'
    }
    $os = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription
    if ($os -match 'Darwin') { if ($arch -eq 'Arm64') { return 'darwin-arm64' }; return 'darwin-x64' }
    if ($arch -eq 'Arm64') { return 'linux-arm64' }
    return 'linux-x64'
}

$Targets = switch ($Platform) {
    'host' { @((Get-HostPlatformKey)) }
    'all' { @($Matrix.Keys) }
    default { @($Platform) }
}
if ($ArtifactRoot) {
    $ArtifactRoot = (Resolve-Path $ArtifactRoot).Path
} elseif ($Platform -eq 'all') {
    throw '-Platform all requires -ArtifactRoot, or stage cross-built targets individually'
}

$ManifestPlatforms = [ordered]@{}
foreach ($Key in $Targets) {
    $Triple = $Matrix[$Key]
    $File = if ($Key -eq 'win32-x64') { 'wayland-nano.exe' } else { 'wayland-nano' }
    # P4 PTY (RC2 wiring pass): the unix PTY host-death sentinel ships next
    # to the host binary (pty.rs resolves it beside current_exe; the
    # NANO_PTY_GUARD_EXE override is the documented escape hatch). Windows
    # needs no guard — ConPTY + Job Objects carry the teardown there.
    # @(...) is mandatory: an if/else expression unpipes a single-element
    # array to a SCALAR, and StrictMode then kills $Helpers.Count (this
    # exact failure broke the first rc.0 unix release legs).
    $Helpers = @(if ($Key -eq 'win32-x64') { @() } else { 'wayland-nano-pty-guard' })
    if ($ArtifactRoot) {
        $Source = Join-Path $ArtifactRoot "$Key\$File"
        $HelperSources = @($Helpers | ForEach-Object { Join-Path $ArtifactRoot "$Key\$_" })
    } else {
        if (-not $SkipBuild) {
            $CargoArgs = @('build', '--release', '-p', 'nano-cli')
            if ($Platform -ne 'host') { $CargoArgs += @('--target', $Triple) }
            Write-Host "==> cargo $($CargoArgs -join ' ')"
            & cargo @CargoArgs
            if ($LASTEXITCODE -ne 0) { throw "cargo build failed for $Key (exit $LASTEXITCODE)" }
            if ($Helpers.Count -gt 0) {
                # The guard binary is a nano-tools bin target.
                $GuardArgs = @('build', '--release', '-p', 'nano-tools', '--bin', 'wayland-nano-pty-guard')
                if ($Platform -ne 'host') { $GuardArgs += @('--target', $Triple) }
                Write-Host "==> cargo $($GuardArgs -join ' ')"
                & cargo @GuardArgs
                if ($LASTEXITCODE -ne 0) { throw "guard build failed for $Key (exit $LASTEXITCODE)" }
            }
        }
        $Source = if ($Platform -eq 'host') {
            Join-Path $RepoRoot "target\release\$File"
        } else {
            Join-Path $RepoRoot "target\$Triple\release\$File"
        }
        $HelperSources = @($Helpers | ForEach-Object {
            if ($Platform -eq 'host') { Join-Path $RepoRoot "target\release\$_" }
            else { Join-Path $RepoRoot "target\$Triple\release\$_" }
        })
    }
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) { throw "expected binary not found: $Source" }
    foreach ($HelperSource in $HelperSources) {
        if (-not (Test-Path -LiteralPath $HelperSource -PathType Leaf)) { throw "expected helper binary not found: $HelperSource" }
    }

    $DestinationDir = Join-Path $PackageRoot "binaries\$Key"
    New-Item -ItemType Directory -Force -Path $DestinationDir | Out-Null
    $Destination = Join-Path $DestinationDir $File
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
    $Info = Get-Item -LiteralPath $Destination
    $HelperManifest = @()
    for ($i = 0; $i -lt $Helpers.Count; $i++) {
        $HelperDestination = Join-Path $DestinationDir $Helpers[$i]
        Copy-Item -LiteralPath $HelperSources[$i] -Destination $HelperDestination -Force
        $HelperInfo = Get-Item -LiteralPath $HelperDestination
        $HelperManifest += [ordered]@{
            file = $Helpers[$i]
            size = $HelperInfo.Length
            sha256 = (Get-FileHash -LiteralPath $HelperDestination -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        Write-Host "==> staged $Key helper $($Helpers[$i]) ($($HelperInfo.Length) bytes)"
    }
    $ManifestPlatforms[$Key] = [ordered]@{
        file = $File
        size = $Info.Length
        sha256 = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
        helpers = $HelperManifest
    }
    Write-Host "==> staged $Key ($($Info.Length) bytes)"
}

$Manifest = [ordered]@{ schema = 1; algorithm = 'sha256'; platforms = $ManifestPlatforms }
$ManifestPath = Join-Path $PackageRoot 'binaries-manifest.json'
$Json = $Manifest | ConvertTo-Json -Depth 5
$Bytes = [System.Text.UTF8Encoding]::new($false).GetBytes("$Json`n")
$Handle = [System.IO.File]::Open($ManifestPath, 'Create', 'Write', 'None')
try { $Handle.Write($Bytes, 0, $Bytes.Length); $Handle.Flush($true) } finally { $Handle.Dispose() }
Write-Host "==> wrote $ManifestPath"
