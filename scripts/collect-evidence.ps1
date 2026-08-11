#requires -Version 5.1
<#
.SYNOPSIS
  Collects the immutable release evidence bundle (docs/release/EVIDENCE-BUNDLE.md).

.DESCRIPTION
  Copies each evidence slot's artifacts into -BundleDir, then writes
  MANIFEST.sha256 pinning the SHA-256 of every collected file, plus a
  bundle.json metadata header. Fails closed: a missing or empty required slot
  aborts with exit 1 and no manifest is written, unless -AllowMissing is
  passed (partial bundle, stamped "sealed": false — diagnostics only, not
  release evidence).

  Copies files verbatim; never reads or prints file contents (no secrets).

.EXAMPLE
  pwsh wayland-nano/scripts/collect-evidence.ps1 -BundleDir .\bundle-v0.1.0-alpha
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,

    # Repo root of wayland-nano (defaults to the script's parent directory).
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,

    # Shared review root (defaults to ../shared relative to wayland-nano).
    [string]$SharedRoot = (Join-Path (Split-Path $RepoRoot -Parent) "shared"),

    # Produce a partial bundle when a slot is missing/empty (stamped unsealed).
    [switch]$AllowMissing
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$slots = @(
    @{ Name = "ci-gate";        Src = (Join-Path $RepoRoot "artifacts/evidence/ci");      Filter = "*"      },
    @{ Name = "c12-manifests";  Src = (Join-Path $RepoRoot "scripts/c12-proof/evidence"); Filter = "*.json" },
    @{ Name = "panel-verdicts"; Src = (Join-Path $SharedRoot "reviews/panel");            Filter = "*"      },
    @{ Name = "canary";         Src = (Join-Path $RepoRoot "artifacts/evidence/canary");  Filter = "*.json" }
)

$missing = @()
$collected = @()  # @{ Src; Dst }

foreach ($slot in $slots) {
    $files = @()
    if (Test-Path $slot.Src) {
        $files = @(Get-ChildItem -Path $slot.Src -Filter $slot.Filter -File -Recurse |
                   Where-Object { $_.Name -notmatch "^\.(git|ds_store)" })
    }
    if ($files.Count -eq 0) {
        $missing += "$($slot.Name) ($($slot.Src))"
        continue
    }
    $dstDir = Join-Path $BundleDir $slot.Name
    New-Item -ItemType Directory -Force -Path $dstDir | Out-Null
    foreach ($f in $files) {
        $dst = Join-Path $dstDir $f.Name
        Copy-Item -LiteralPath $f.FullName -Destination $dst -Force
        $collected += @{ Src = $f.FullName; Dst = $dst }
    }
    Write-Host "slot $($slot.Name): $($files.Count) file(s)"
}

if ($missing.Count -gt 0) {
    foreach ($m in $missing) { Write-Warning "missing/empty evidence slot: $m" }
    if (-not $AllowMissing) {
        throw "required evidence slots missing; bundle NOT written (use -AllowMissing for a partial diagnostic bundle)"
    }
}

if ($collected.Count -eq 0) {
    throw "no evidence collected at all; refusing to write an empty bundle"
}

# bundle.json metadata header
$gitSha = $null
try {
    $gitSha = (git -C $RepoRoot rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0) { $gitSha = $null }
} catch { $gitSha = $null }

$bundleMeta = [ordered]@{
    bundle   = "wayland-nano-release-evidence"
    version  = 1
    sealed   = ($missing.Count -eq 0)
    at       = (Get-Date).ToUniversalTime().ToString("o")
    repoRoot = $RepoRoot
    gitSha   = $gitSha
    slots    = $slots.Name
    missing  = $missing
    files    = $collected.Count
}
$bundleJsonPath = Join-Path $BundleDir "bundle.json"
$bundleMeta | ConvertTo-Json | Out-File $bundleJsonPath -Encoding utf8

# MANIFEST.sha256 — every file in the bundle, including bundle.json.
# LF endings (never CRLF) so `sha256sum -c` parses it on any platform.
$manifestPath = Join-Path $BundleDir "MANIFEST.sha256"
$lines = @()
$allFiles = @($collected | ForEach-Object { $_.Dst }) + $bundleJsonPath
foreach ($p in ($allFiles | Sort-Object)) {
    $hash = (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant()
    $rel = [System.IO.Path]::GetRelativePath((Resolve-Path $BundleDir).Path, (Resolve-Path $p).Path) -replace "\\", "/"
    $lines += "$hash  $rel"
}
[System.IO.File]::WriteAllText($manifestPath, ($lines -join "`n") + "`n")

Write-Host "bundle: $BundleDir ($($collected.Count) evidence file(s), sealed=$($missing.Count -eq 0))"
Write-Host "verify: cd $BundleDir; sha256sum -c MANIFEST.sha256"
