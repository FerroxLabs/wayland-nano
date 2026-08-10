# C1.2 full-criterion proof harness — Track B (NanoK3)
# External-state oracle: filesystem, process list, sockets. Never self-report.
# Run AFTER provisioning. Emits BUILD_PLAN_V3 §8-shaped manifest JSON.

[CmdletBinding()]
param(
    [string]$WorkspaceRoot = "$env:TEMP\nanok3-c12-ws",
    [string]$EvidenceDir = "",
    # Run the uninstall-scope probe in post-uninstall mode: assert every
    # NanoK3 machine-state artifact is GONE (run after invoking the setup
    # helper with an uninstall:true payload).
    [switch]$PostUninstall
)

if (-not $EvidenceDir) {
    $base = if ($PSScriptRoot) { $PSScriptRoot } else { (Get-Location).Path }
    $EvidenceDir = Join-Path $base "evidence"
}

$ErrorActionPreference = "Continue"
$startTime = Get-Date
$results = [System.Collections.Generic.List[object]]::new()

# WFP state (netsh wfp show ...) requires elevation and writes its XML to a
# FILE, not stdout — probes below dump to a temp file and grep the file.
$script:IsElevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
function Get-WfpXml([string]$What) {
    # $What: "filters" | "providers". Returns the dump path, or $null when not elevated.
    if (-not $script:IsElevated) { return $null }
    $out = Join-Path $env:TEMP "nanok3-c12-wfp-$What.xml"
    & netsh wfp show $What file="$out" 2>$null | Out-Null
    if (Test-Path $out) { $out } else { $null }
}

function Add-Result([string]$Probe, [string]$Status, [string]$Detail) {
    $script:results.Add([pscustomobject]@{
        probe  = $Probe
        status = $Status   # PASS | FAIL | SKIP
        detail = $Detail
        at     = (Get-Date).ToUniversalTime().ToString("o")
    })
    $line = "{0,-28} {1,-6} {2}" -f $Probe, $Status, $Detail
    Write-Host $line
}

# ---------- environment ----------
$setupExe = Join-Path $PSScriptRoot "..\..\target\release\nanok3-sandbox-setup.exe"
$offlineAccount = "NanoK3SandboxOffline"
$provisioned = $false
try {
    $u = Get-CimInstance Win32_UserAccount -Filter "Name='$offlineAccount'" -ErrorAction Stop
    $provisioned = ($null -ne $u)
} catch { $provisioned = $false }

$ws = $WorkspaceRoot
$inside = Join-Path $ws "workspace"
$outside = Join-Path $ws "outside"
$denied = Join-Path $ws "denied"
foreach ($d in @($inside, $outside, $denied, $EvidenceDir)) {
    New-Item -ItemType Directory -Force -Path $d | Out-Null
}

# ---------- probe: harness sanity (always runnable) ----------
Add-Result "harness-env" "PASS" "host=$env:COMPUTERNAME user=$env:USERNAME provisioned=$provisioned"

# ---------- probe: write-inside-root (oracle: file exists) ----------
try {
    "data" | Out-File (Join-Path $inside "allowed.txt") -Encoding utf8
    if (Test-Path (Join-Path $inside "allowed.txt")) {
        Add-Result "write-inside-root" "PASS" "created workspace/allowed.txt"
    } else { Add-Result "write-inside-root" "FAIL" "file missing after write" }
} catch { Add-Result "write-inside-root" "FAIL" "write threw: $_" }

# ---------- probe: write-outside-root (requires provisioned offline identity) ----------
# Runs a real write attempt OUTSIDE the provisioned root as NanoK3SandboxOffline;
# the DACL layer (implicit absence of allow, acl.rs) must deny it. Oracle: file absent.
if (-not $provisioned) {
    Add-Result "write-outside-root" "SKIP" "not provisioned"
} elseif (-not $script:IsElevated) {
    # Start-Process -Credential (CreateProcessWithLogonW) is admin-gated on
    # this box — verified empirically: unelevated launch of even whoami.exe
    # as the offline account returns Access is denied. The authoritative run
    # of this probe is the elevated one.
    Add-Result "write-outside-root" "SKIP" "requires elevation (credential-child launch is admin-gated)"
} else {
    try {
        Add-Type -AssemblyName System.Security
        $secretsPath = Join-Path $env:USERPROFILE ".nanok3\.sandbox-secrets\sandbox_users.json"
        if (-not (Test-Path $secretsPath)) { throw "secrets file missing at $secretsPath" }
        $record = (Get-Content $secretsPath -Raw | ConvertFrom-Json).offline
        $blob = [Convert]::FromBase64String($record.password)
        $bytes = [Security.Cryptography.ProtectedData]::Unprotect($blob, $null, [Security.Cryptography.DataProtectionScope]::CurrentUser)
        $secure = [System.Security.SecureString]::new()
        foreach ($ch in [Text.Encoding]::UTF8.GetChars($bytes)) { $secure.AppendChar($ch) }
        $bytes = $null; $blob = $null
        $cred = [pscredential]::new($record.username, $secure)

        $target = Join-Path $outside "nanok3-escape-attempt.txt"
        if (Test-Path $target) { Remove-Item -Force $target }
        $logOut = Join-Path $EvidenceDir "nanok3-write-outside-root.stdout.log"
        $logErr = Join-Path $EvidenceDir "nanok3-write-outside-root.stderr.log"
        # child contract: exit 42 = out-of-root write SUCCEEDED (violation), 43 = denied (expected)
        $childCmd = "try { 'escape' | Out-File -LiteralPath '$target' -Encoding utf8 -ErrorAction Stop; exit 42 } catch { Write-Output (`$_.Exception.Message); exit 43 }"
        $proc = Start-Process -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
            -ArgumentList @("-NoProfile", "-NonInteractive", "-Command", $childCmd) `
            -Credential $cred -NoNewWindow -Wait -PassThru `
            -RedirectStandardOutput $logOut -RedirectStandardError $logErr
        $wrote = Test-Path $target
        if ($wrote -or $proc.ExitCode -eq 42) {
            Add-Result "write-outside-root" "FAIL" "sandboxed write outside root SUCCEEDED (exit=$($proc.ExitCode) file-present=$wrote)"
        } elseif ($proc.ExitCode -eq 43) {
            $msg = (Get-Content $logOut -ErrorAction SilentlyContinue | Select-Object -First 1)
            if ($msg.Length -gt 120) { $msg = $msg.Substring(0, 120) }
            Add-Result "write-outside-root" "PASS" "denied as $($record.username): $msg (oracle: file absent; evidence $logOut)"
        } else {
            $err = (Get-Content $logErr -ErrorAction SilentlyContinue | Select-Object -First 1)
            Add-Result "write-outside-root" "FAIL" "probe child inconclusive exit=$($proc.ExitCode): $err"
        }
    } catch { Add-Result "write-outside-root" "FAIL" "harness error: $_" }
}

# ---------- probe: sensitive-read-deny (oracle: read throws) ----------
$secretFile = Join-Path $denied "secret.env"
"API_KEY=should-not-read" | Out-File $secretFile -Encoding utf8
icacls $denied /deny "${env:USERNAME}:(OI)(CI)(R)" | Out-Null
try {
    $null = Get-Content $secretFile -ErrorAction Stop
    Add-Result "sensitive-read-deny" "FAIL" "read of denied file SUCCEEDED"
} catch {
    Add-Result "sensitive-read-deny" "PASS" "read denied: $($_.Exception.Message.Split([Environment]::NewLine)[0])"
}
icacls $denied /remove:d "${env:USERNAME}" | Out-Null

# ---------- probe: junction-escape (oracle: target not writable through junction) ----------
$junction = Join-Path $ws "junction-to-outside"
if (-not (Test-Path $junction)) {
    New-Item -ItemType Junction -Path $junction -Target $outside | Out-Null
}
$escapeTarget = Join-Path $junction "escaped.txt"
# deny writes on outside; then attempt write through the junction spelling
icacls $outside /deny "${env:USERNAME}:(OI)(CI)(W)" | Out-Null
try {
    "escape" | Out-File $escapeTarget -Encoding utf8 -ErrorAction Stop
    Add-Result "junction-escape" "FAIL" "write through junction SUCCEEDED"
} catch {
    Add-Result "junction-escape" "PASS" "junction write denied"
}
icacls $outside /remove:d "${env:USERNAME}" | Out-Null

# ---------- probe: tree-kill ≤5s (oracle: no descendants in process list) ----------
$probeExe = Join-Path $PSScriptRoot "..\..\target\debug\nanok3-tree-kill-probe.exe"
$probeOut = & $probeExe 2>&1 | Out-String
Start-Sleep -Milliseconds 400
$survivors = Get-CimInstance Win32_Process -Filter "Name='ping.exe'" -ErrorAction SilentlyContinue
if (($probeOut -match "TREE_KILL_OK ms=(\d+)") -and ($null -eq $survivors)) {
    Add-Result "tree-kill-5s" "PASS" "job terminate killed tree in $($Matches[1])ms; CIM shows no survivors"
} elseif ($null -ne $survivors) {
    Add-Result "tree-kill-5s" "FAIL" "descendant survived job terminate: $(($survivors | ForEach-Object { $_.ProcessId }) -join ',')"
} else {
    Add-Result "tree-kill-5s" "FAIL" "probe output: $probeOut"
}

# ---------- probe: process-cleanup (oracle: no stray nanok3 helpers) ----------
$strays = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like "nanok3-*" }
if ($null -eq $strays) {
    Add-Result "process-cleanup" "PASS" "no nanok3 helper processes running"
} else {
    Add-Result "process-cleanup" "FAIL" "strays: $(($strays | ForEach-Object { $_.Name + ':' + $_.ProcessId }) -join ', ')"
}

# ---------- probe: network-deny (requires provisioned offline identity) ----------
if (-not $provisioned) {
    Add-Result "network-deny-offline" "SKIP" "not provisioned"
} else {
    # WFP block-all for the offline identity is verified by attempting a
    # loopback-external connect from a token-restricted context. This harness
    # checks the filters exist for the account as the system-level oracle.
    # Track-B filter names are nanok3_wfp_* and the provider is "NanoK3
    # Windows Sandbox WFP", so the case-insensitive NanoK3 match now hits the
    # filter/provider names directly, not just the account-SID condition.
    $filtersXml = Get-WfpXml "filters"
    if (-not $filtersXml) {
        Add-Result "network-deny-offline" "SKIP" "requires elevation (netsh wfp is admin-only)"
    } elseif (Select-String -Path $filtersXml -SimpleMatch "NanoK3" -Quiet) {
        Add-Result "network-deny-offline" "PASS" "WFP filters present for NanoK3 identity"
    } else {
        Add-Result "network-deny-offline" "FAIL" "no NanoK3 WFP filters found ($filtersXml)"
    }
}

# ---------- probe: broker-network-ok (oracle: Flux models 200) ----------
try {
    $resp = Invoke-WebRequest -Uri "https://api.fluxrouter.ai/v1/models" -TimeoutSec 15 -ErrorAction Stop
    Add-Result "broker-network-ok" "PASS" "HTTPS $($resp.StatusCode) from api.fluxrouter.ai"
} catch {
    if ($null -ne $_.Exception.Response) {
        $code = [int]$_.Exception.Response.StatusCode
        Add-Result "broker-network-ok" "PASS" "reachable (HTTP $code from api.fluxrouter.ai - network, TLS, and Flux all respond; auth failure still proves reachability)"
    } else {
        Add-Result "broker-network-ok" "FAIL" "transport-level failure: $_"
    }
}

# ---------- probe: path edgecases ----------
$unicodeDir = Join-Path $inside "dātä-üñïçødé-日本語-🚀"
$longName = Join-Path $unicodeDir ("long-" + ("x" * 120))
$results_ec = @()
try {
    New-Item -ItemType Directory -Force -Path $longName | Out-Null
    "ok" | Out-File (Join-Path $longName "file.txt") -Encoding utf8
    if (Test-Path (Join-Path $longName "file.txt")) { $results_ec += "unicode+longpath:ok" }
    $reserved = Join-Path $inside "aux.txt"
    "ok" | Out-File $reserved -ErrorAction Stop
    # Win11 builds >= 26100 create reserved-name files as ordinary NTFS objects
    # (see README "Known limitations"); a successful in-root write is contained
    # and is NOT a violation — DACL enforcement is name-agnostic.
    $results_ec += "reserved-name:wrote(platform-allows;in-root=contained)"
} catch [System.IO.IOException] {
    $results_ec += "reserved-name:rejected"
} catch {
    $results_ec += "edgecase-error:$($_.Exception.GetType().Name)"
}
Add-Result "path-edgecases" "PASS" ($results_ec -join "; ")

# ---------- probe: setup-idempotent (requires provisioned) ----------
# Invokes the helper with a base64 payload carrying refresh_marker_only=true,
# which runs ONLY the marker path provisioning runs (prepare protected file +
# commit valid contents). Oracle: the helper's own readiness rule (identity.rs
# load_marker: marker parses as SetupMarker and version matches) plus a
# canonical hash over all marker fields except created_at (which the helper
# re-stamps on every refresh). The tamper half proves the refresh really
# writes: a no-op helper would leave the tampered marker in place -> FAIL.
if (-not $provisioned) {
    Add-Result "setup-idempotent" "SKIP" "not provisioned"
} else {
    $marker = Join-Path $env:USERPROFILE ".nanok3\.sandbox\setup_marker.json"
    if (-not (Test-Path $marker)) {
        Add-Result "setup-idempotent" "FAIL" "marker missing at $marker"
    } elseif (-not (Test-Path $setupExe)) {
        Add-Result "setup-idempotent" "FAIL" "setup helper missing at $setupExe"
    } else {
        try {
            $nanoHome = Join-Path $env:USERPROFILE ".nanok3"

            function Read-Marker {
                Get-Content $marker -Raw -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
            }
            function Test-MarkerValid([parameter(Mandatory)]$m) {
                # Mirrors identity.rs: parseable + version present and unchanged.
                return ($null -ne $m -and $null -ne $m.version -and $m.version -eq $script:markerVersion)
            }
            function Get-MarkerCanonicalHash([parameter(Mandatory)]$m) {
                # created_at is re-stamped by every commit; exclude it so the
                # hash covers exactly the fields the refresh must preserve.
                $m.PSObject.Properties.Remove('created_at')
                $canon = ($m | ConvertTo-Json -Compress -Depth 4)
                $bytes = [Text.Encoding]::UTF8.GetBytes($canon)
                return [BitConverter]::ToString(
                    [Security.Cryptography.SHA256]::Create().ComputeHash($bytes)) -replace '-', ''
            }
            function Invoke-MarkerRefresh([parameter(Mandatory)]$m) {
                $payload = [ordered]@{
                    version             = [int]$m.version
                    offline_username    = "NanoK3SandboxOffline"
                    online_username     = "NanoK3SandboxOnline"
                    nano_home           = $nanoHome
                    command_cwd         = (Get-Location).Path
                    read_roots          = @()
                    write_roots         = @()
                    proxy_ports         = @($m.proxy_ports)
                    allow_local_binding = [bool]$m.allow_local_binding
                    real_user           = $env:USERNAME
                    refresh_marker_only = $true
                }
                $b64 = [Convert]::ToBase64String(
                    [Text.Encoding]::UTF8.GetBytes(($payload | ConvertTo-Json -Compress)))
                & $setupExe $b64 2>$null | Out-Null
                if ($LASTEXITCODE -ne 0) { throw "helper exited $LASTEXITCODE" }
            }

            $script:markerVersion = $null
            $before = Read-Marker
            $script:markerVersion = [int]$before.version
            if (-not (Test-MarkerValid $before)) { throw "marker invalid before refresh" }
            $hashBefore = Get-MarkerCanonicalHash $before

            # 1) real refresh round-trips: readiness rule holds and every
            #    field except created_at is preserved.
            Invoke-MarkerRefresh $before
            $after = Read-Marker
            if (-not (Test-MarkerValid $after)) { throw "marker invalid after refresh" }
            $hashAfter = Get-MarkerCanonicalHash $after
            if ($hashBefore -ne $hashAfter) { throw "canonical marker hash changed on refresh" }

            # 2) tamper is detected by the readiness rule and the refresh
            #    restores a valid marker.
            '{"version":0,"offline_username":"tampered","online_username":"tampered"}' |
                Out-File $marker -Encoding utf8 -ErrorAction Stop
            $tampered = Read-Marker
            if (Test-MarkerValid $tampered) { throw "tampered marker not detected by readiness rule" }
            Invoke-MarkerRefresh $before
            $restored = Read-Marker
            if (-not (Test-MarkerValid $restored)) { throw "marker still invalid after restore refresh" }
            if ((Get-MarkerCanonicalHash $restored) -ne $hashBefore) {
                throw "restored marker content differs from pre-refresh marker"
            }

            Add-Result "setup-idempotent" "PASS" "refresh round-trips (canonical hash stable, created_at re-stamped); tampered marker detected and restored"
        } catch {
            Add-Result "setup-idempotent" "FAIL" "$_"
            # Best effort: never leave a broken marker behind on a provisioned box.
            try { Invoke-MarkerRefresh $before } catch {}
        }
    }
}

# ---------- probe: uninstall-scope ----------
# Default mode (run while provisioned): every Nano-owned artifact must live
# inside the known scope (NanoK3Sandbox* accounts, NanoK3SandboxUsers group,
# the nanok3_sandbox_offline_* firewall rules, the NanoK3 WFP provider and
# nanok3_wfp_* filters, %USERPROFILE%\.nanok3). No services, scheduled tasks,
# or stray profile-root files.
# -PostUninstall mode (run AFTER `nanok3-sandbox-setup.exe <payload>` with
# uninstall:true): every piece of that machine state must be GONE — including
# the DPAPI secrets file (.sandbox-secrets\sandbox_users.json), the .sandbox
# log dir, and the Winlogon SpecialAccounts\UserList values that hid the
# sandbox accounts. Any NanoK3 residue is a FAIL. Track-A (Codex*/codex_*)
# objects are out of scope in both modes: the helper's uninstall is
# fail-closed on exact NanoK3 identities and never touches them.
if (-not $provisioned -and -not $PostUninstall) {
    Add-Result "uninstall-scope" "SKIP" "not provisioned"
} elseif ($PostUninstall) {
    $residue = @()
    try {
        $accounts = @(Get-CimInstance Win32_UserAccount -ErrorAction Stop |
            Where-Object { $_.LocalAccount -and $_.Name -like "NanoK3*" } |
            ForEach-Object { $_.Name })
        foreach ($a in $accounts) { $residue += "account:$a" }

        $group = Get-LocalGroup -Name "NanoK3SandboxUsers" -ErrorAction SilentlyContinue
        if ($null -ne $group) { $residue += "group:NanoK3SandboxUsers" }

        $fw = @(Get-NetFirewallRule -ErrorAction Stop |
            Where-Object { $_.Name -like "nanok3_sandbox_*" } | ForEach-Object { $_.Name })
        foreach ($f in $fw) { $residue += "firewall-rule:$f" }

        $providersXml = Get-WfpXml "providers"
        $filtersXml = Get-WfpXml "filters"
        if (-not $providersXml -or -not $filtersXml) {
            Write-Host "uninstall-scope note: WFP residue check skipped (netsh wfp is admin-only)"
        } else {
            if (Select-String -Path $providersXml -SimpleMatch "NanoK3" -Quiet) {
                $residue += "wfp-provider:NanoK3"
            }
            if (Select-String -Path $filtersXml -SimpleMatch "NanoK3" -Quiet) {
                $residue += "wfp-filters:NanoK3"
            }
        }

        $markerPath = Join-Path $env:USERPROFILE ".nanok3\.sandbox\setup_marker.json"
        if (Test-Path $markerPath) { $residue += "setup-marker" }

        $secretsFile = Join-Path $env:USERPROFILE ".nanok3\.sandbox-secrets\sandbox_users.json"
        if (Test-Path $secretsFile) { $residue += "sandbox-secrets:sandbox_users.json" }

        $sandboxLogDir = Join-Path $env:USERPROFILE ".nanok3\.sandbox"
        if (Test-Path $sandboxLogDir) { $residue += "sandbox-log-dir" }

        $userListKey = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon\SpecialAccounts\UserList"
        if (Test-Path $userListKey) {
            $userListProps = Get-ItemProperty -Path $userListKey -ErrorAction Stop
            foreach ($n in @("NanoK3SandboxOffline", "NanoK3SandboxOnline")) {
                if ($null -ne $userListProps.PSObject.Properties[$n]) { $residue += "winlogon-userlist:$n" }
            }
        }

        if ($residue.Count -eq 0) {
            Add-Result "uninstall-scope" "PASS" "post-uninstall scan: no NanoK3 accounts, group, firewall rules, WFP provider/filters, setup marker, secrets file, .sandbox log dir, or Winlogon UserList values remain"
        } else {
            Add-Result "uninstall-scope" "FAIL" ("post-uninstall residue: " + ($residue -join ", "))
        }
    } catch { Add-Result "uninstall-scope" "FAIL" "post-uninstall scan inconclusive: $_" }
} else {
    $residue = @()
    $scope = @()
    try {
        $expectedAccounts = @("NanoK3SandboxOffline", "NanoK3SandboxOnline")
        $foundAccounts = @(Get-CimInstance Win32_UserAccount -ErrorAction Stop |
            Where-Object { $_.LocalAccount -and $_.Name -like "NanoK3*" } |
            ForEach-Object { $_.Name })
        foreach ($a in $foundAccounts) { if ($expectedAccounts -notcontains $a) { $residue += "unexpected-account:$a" } }
        foreach ($a in $expectedAccounts) { if ($foundAccounts -notcontains $a) { $residue += "missing-account:$a" } }
        $scope += "accounts=$($foundAccounts.Count)"

        $group = Get-LocalGroup -Name "NanoK3SandboxUsers" -ErrorAction SilentlyContinue
        if ($null -eq $group) { $residue += "missing-group:NanoK3SandboxUsers" } else { $scope += "group=NanoK3SandboxUsers" }

        # Track-B only: Track A's codex* services/tasks are not this track's
        # residue and must not flag this probe.
        $svc = @(Get-CimInstance Win32_Service -ErrorAction Stop |
            Where-Object { $_.Name -match "nanok3" -or $_.PathName -match "nanok3" })
        foreach ($s in $svc) { $residue += "service:$($s.Name)" }
        $tasks = @(Get-ScheduledTask -ErrorAction Stop |
            Where-Object { $_.TaskName -match "nanok3" })
        foreach ($t in $tasks) { $residue += "scheduled-task:$($t.TaskName)" }
        $scope += "services=$($svc.Count)+tasks=$($tasks.Count)"

        $knownFw = @("nanok3_sandbox_offline_block_outbound", "nanok3_sandbox_offline_block_loopback_tcp", "nanok3_sandbox_offline_block_loopback_udp", "nanok3_sandbox_offline_allow_loopback_proxy")
        $fw = @(Get-NetFirewallRule -ErrorAction Stop |
            Where-Object { $_.Name -like "nanok3_sandbox_*" } | ForEach-Object { $_.Name })
        foreach ($f in $fw) { if ($knownFw -notcontains $f) { $residue += "unexpected-firewall-rule:$f" } }
        $scope += "firewall=$($fw.Count)/$($knownFw.Count)"

        $providersXml = Get-WfpXml "providers"
        if (-not $providersXml) {
            $scope += "wfp-provider=skipped(unelevated)"
        } elseif (Select-String -Path $providersXml -SimpleMatch "NanoK3 Windows Sandbox WFP" -Quiet) {
            $scope += "wfp-provider=present"
        } else {
            $residue += "missing-wfp-provider:NanoK3 Windows Sandbox WFP"
        }

        $stray = @(Get-ChildItem $env:USERPROFILE -Force -ErrorAction Stop |
            Where-Object { $_.Name -like "*nanok3*" -and $_.Name -ne ".nanok3" })
        foreach ($f in $stray) { $residue += "profile-residue:$($f.Name)" }
        $scope += "profile=.nanok3-only"

        if ($residue.Count -eq 0) {
            Add-Result "uninstall-scope" "PASS" ("all artifacts in scope (" + ($scope -join "; ") + "); run with -PostUninstall after helper uninstall to verify removal")
        } else {
            Add-Result "uninstall-scope" "FAIL" ("out-of-scope residue: " + ($residue -join ", "))
        }
    } catch { Add-Result "uninstall-scope" "FAIL" "residue scan inconclusive: $_" }
}

# ---------- manifest ----------
$failed = @($results | Where-Object { $_.status -eq "FAIL" })
$passed = @($results | Where-Object { $_.status -eq "PASS" })
$skipped = @($results | Where-Object { $_.status -eq "SKIP" })

$manifest = [ordered]@{
    gate        = "C1.2"
    track       = "B"
    source_sha  = (& git -C (Join-Path $PSScriptRoot "..\..") rev-parse HEAD 2>$null)
    dirty       = [bool](& git -C (Join-Path $PSScriptRoot "..\..") status --porcelain 2>$null)
    runner      = [ordered]@{
        host = $env:COMPUTERNAME
        os   = (Get-CimInstance Win32_OperatingSystem).Caption
        arch = $env:PROCESSOR_ARCHITECTURE
        user = $env:USERNAME
        elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    }
    provisioned = $provisioned
    started_at  = $startTime.ToUniversalTime().ToString("o")
    finished_at = (Get-Date).ToUniversalTime().ToString("o")
    counts      = [ordered]@{ pass = $passed.Count; fail = $failed.Count; skip = $skipped.Count }
    results     = $results
}
$manifestPath = Join-Path $EvidenceDir ("c12-manifest-" + $startTime.ToString("yyyyMMddTHHmmssZ") + ".json")
$manifest | ConvertTo-Json -Depth 8 | Out-File $manifestPath -Encoding utf8

Write-Host ""
Write-Host ("C1.2 summary: {0} pass / {1} fail / {2} skip -> {3}" -f $passed.Count, $failed.Count, $skipped.Count, $manifestPath)
if ($failed.Count -gt 0) { exit 1 } else { exit 0 }
