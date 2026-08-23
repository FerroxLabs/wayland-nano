# Human-like acceptance harness for Wayland Nano.
# Drives the real binary the way a user would: CLI contract, exec-mode Q&A,
# file-write permission behavior, kill-resume, sessions, metering.
# Credential: FLUX_API_KEY_FILE is set to the governed key PATH by the caller;
# the value is never read, echoed, or logged.
param(
  [string]$Bin = 'F:\CargoTarget\wayland-nano\debug\wayland-nano.exe',
  [string]$Root = 'F:\tmp\human-harness'
)
$ErrorActionPreference = 'Stop'
$results = [System.Collections.Generic.List[string]]::new()
function Record($leg, $ok, $detail) { $script:results.Add(("{0} {1} — {2}" -f ($(if($ok){'PASS'}else{'FAIL'}), $leg, $detail))) }
function Fail($leg, $detail) { Record $leg $false $detail; throw "$leg failed: $detail" }

# --- Leg 0: CLI contract -------------------------------------------------------
$ver = & $Bin --version 2>&1
if ($LASTEXITCODE -ne 0 -or $ver -notmatch '^wayland-nano \d+\.\d+\.\d+') { Fail 'L0-version' "unexpected: $ver" }
Record 'L0-version' $true $ver

$bare = & $Bin 2>&1
if ($LASTEXITCODE -ne 2 -or "$bare" -notmatch 'usage:') { Fail 'L0-bare' "exit=$LASTEXITCODE out=$bare" }
Record 'L0-bare' $true 'usage printed, exit 2'

# --- Environment for live legs --------------------------------------------------
if (-not $env:FLUX_API_KEY_FILE) { Fail 'env' 'FLUX_API_KEY_FILE not set (path only)' }
if (-not (Test-Path $env:FLUX_API_KEY_FILE)) { Fail 'env' 'key file path does not resolve' }
if (Test-Path $Root) { Remove-Item -Recurse -Force $Root }
$workspace = "$Root\workspace"; $nanoHome = "$Root\nano-home"
New-Item -ItemType Directory -Force $workspace, $nanoHome | Out-Null

$env:NANO_HOME = $nanoHome
$common = @{ }
function Invoke-Nano([string]$argString, $timeoutSec) {
  $p = Start-Process -FilePath $Bin -ArgumentList $argString -WorkingDirectory $workspace `
    -RedirectStandardOutput "$Root\out.txt" -RedirectStandardError "$Root\err.txt" `
    -PassThru -NoNewWindow
  if (-not $p.WaitForExit($timeoutSec * 1000)) { $p.Kill($true); $p.WaitForExit(); return @{ exit = 124; out = (Get-Content "$Root\out.txt" -Raw -ErrorAction SilentlyContinue); err = (Get-Content "$Root\err.txt" -Raw -ErrorAction SilentlyContinue); killed = $true } }
  return @{ exit = $p.ExitCode; out = (Get-Content "$Root\out.txt" -Raw -ErrorAction SilentlyContinue); err = (Get-Content "$Root\err.txt" -Raw -ErrorAction SilentlyContinue); killed = $false }
}

# --- Leg 1: exec Q&A (live Flux turn) -------------------------------------------
$r = Invoke-Nano 'exec "Reply with exactly the token HARNESS_OK and nothing else."' 90
if ($r.killed) { Fail 'L1-exec' 'timed out at 90s' }
if ("$($r.out)$($r.err)" -notmatch 'HARNESS_OK') { Fail 'L1-exec' "no HARNESS_OK in output (exit=$($r.exit)): $($r.err)" }
Record 'L1-exec' $true "exit=$($r.exit), token present"

# --- Leg 2a: default mode denies writes (fail-closed, non-interactive) -----------
$r = Invoke-Nano 'exec "Create a file called denied-note.txt containing the text SHOULD_NOT_EXIST, then stop."' 120
$denied = -not (Test-Path "$workspace\denied-note.txt")
$deniedSeen = ("$($r.out)$($r.err)" -match 'approval_denied')
Record 'L2a-default-denies' ($denied -and $deniedSeen) "no file=$denied; approval_denied seen=$deniedSeen"

# --- Leg 2b: full_auto writes (the human-approved equivalent) ---------------------
$r = Invoke-Nano 'exec --mode full_auto "Create a file called human-note.txt in the current directory containing the text WROTE_BY_NANO, then stop."' 120
$wrote = Test-Path "$workspace\human-note.txt"
$content = if ($wrote) { Get-Content "$workspace\human-note.txt" -Raw } else { '' }
Record 'L2-write' ($wrote -and $content -match 'WROTE_BY_NANO') "wrote=$wrote exit=$($r.exit) err=$($r.err)".Substring(0, [Math]::Min(160, "wrote=$wrote exit=$($r.exit) err=$($r.err)".Length))

# --- Leg 3: sessions inventory ----------------------------------------------------
$r = Invoke-Nano 'sessions' 30
if ($r.exit -ne 0 -or "$($r.out)$($r.err)".Length -lt 4) { Fail 'L3-sessions' "exit=$($r.exit)" }
Record 'L3-sessions' $true 'sessions listed without error'

# --- Leg 4: kill-resume fidelity ---------------------------------------------------
$before = @(Get-ChildItem -Recurse $nanoHome -Filter '*.jsonl' -ErrorAction SilentlyContinue | ForEach-Object FullName)
$p = Start-Process -FilePath $Bin -ArgumentList 'exec "Write a long detailed essay about the history of computing, at least 800 words."' -WorkingDirectory $workspace -RedirectStandardOutput "$Root\l4-out.txt" -RedirectStandardError "$Root\l4-err.txt" -PassThru -NoNewWindow
$killedMidTurn = $false
$killedJournal = $null
for ($i = 0; $i -lt 300 -and -not $p.HasExited; $i++) {
  $candidate = Get-ChildItem -Recurse $nanoHome -Filter '*.jsonl' -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.FullName } | Select-Object -First 1
  if ($candidate -and (Select-String -Path $candidate.FullName -Pattern '"turn_begin"' -Quiet) -and -not (Select-String -Path $candidate.FullName -Pattern '"turn_end"' -Quiet)) {
    $p.Kill($true); $p.WaitForExit(5000); $killedMidTurn = $p.HasExited; $killedJournal = $candidate; break
  }
  Start-Sleep -Milliseconds 50
}
if (-not $p.HasExited) { $p.Kill($true) }
$p.WaitForExit()
$partial = $false
if ($killedJournal) {
  $hasStart = [bool](Select-String -Path $killedJournal.FullName -Pattern '"turn_begin"' -Quiet)
  $hasEnd = [bool](Select-String -Path $killedJournal.FullName -Pattern '"turn_end"' -Quiet)
  $partial = $hasStart -and (-not $hasEnd)
}
Record 'L4-kill-resume' ($killedMidTurn -and $partial) "killed between turn_begin/turn_end=$killedMidTurn; partial journal=$partial"

# --- Leg 5: metering ---------------------------------------------------------------
$journal = Get-ChildItem -Recurse $nanoHome -Filter '*.jsonl' -ErrorAction SilentlyContinue | Where-Object { Select-String -Path $_.FullName -Pattern '"turn_end"' -Quiet } | Sort-Object LastWriteTime | Select-Object -Last 1
$meterSeen = $false
if ($journal) { $meterSeen = [bool](Select-String -Path $journal.FullName -Pattern 'turn_completed|usage|TurnEnd|session_tokens|microcents' -Quiet) }
Record 'L5-meter' $meterSeen $(if ($journal) { $journal.Name } else { 'no journal' })

# --- Report -------------------------------------------------------------------------
$failed = @($results | Where-Object { $_ -like 'FAIL*' })
''
$results | ForEach-Object { $_ }
''
if ($failed.Count) { "HARNESS: $($failed.Count) leg(s) FAILED"; exit 1 } else { 'HARNESS: all legs PASS'; exit 0 }
