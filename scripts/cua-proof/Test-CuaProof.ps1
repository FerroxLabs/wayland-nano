# WP-0.1 CUA live proof runner — owner/host-run (interactive desktop only).
# Evidence: scripts/cua-proof/evidence/cua-manifest-<timestamp>.json
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Set-Location $repo

$helper = 'F:\CargoTarget\wayland-nano\debug\examples\cua_probe_window.exe'
if ($env:CARGO_TARGET_DIR) { $helper = Join-Path $env:CARGO_TARGET_DIR 'debug\examples\cua_probe_window.exe' }

'== build helper + live test =='
cargo build -p nano-cua --example cua_probe_window 2>&1 | Select-Object -Last 1
cargo test -p nano-cua --test live --no-run 2>&1 | Select-Object -Last 1
if (-not (Test-Path $helper)) { throw "probe helper missing at $helper" }

$env:NANO_CUA_LIVE = '1'
$env:NANO_CUA_PROBE_WINDOW = (Resolve-Path $helper).Path

'== live proof: focus invariance + SendInput landing =='
$out = cargo test -p nano-cua --test live windows_focus_invariance_and_sendinput_landing -- --exact --nocapture 2>&1
$out | ForEach-Object { $_ }
if ($LASTEXITCODE -ne 0) { throw 'live proof FAILED' }
if ("$out" -match 'SKIP') { throw 'proof skipped — that is not proof' }

$evidenceDir = Join-Path $PSScriptRoot 'evidence'
New-Item -ItemType Directory -Force $evidenceDir | Out-Null
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$manifest = [ordered]@{
  probe = 'cua-focus-landing'
  utc = $stamp
  helper_sha256 = (Get-FileHash $helper -Algorithm SHA256).Hash
  rustc = (rustc --version)
  applied_dpi = (Get-ItemProperty 'HKCU:\Control Panel\Desktop\WindowMetrics' -Name AppliedDPI -ErrorAction SilentlyContinue).AppliedDPI
  test_output_tail = ($out | Select-Object -Last 4)
  result = 'green'
}
$manifest | ConvertTo-Json | Set-Content (Join-Path $evidenceDir "cua-manifest-$stamp.json") -Encoding utf8
"evidence: $evidenceDir\cua-manifest-$stamp.json"
'CUA-PROOF: PASS'
