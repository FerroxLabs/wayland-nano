[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Initialize', 'Check', 'Closure')]
    [string]$Mode
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$Baseline = 'd8702f22f76aac7dc2d7fcc77b34e4482557ee12'
$PhaseRelative = '.planning/phases/03-wp-0.3-pdf-intake'
$ControlRelative = "$PhaseRelative/03-CONTROL.json"
$RequiredEvidence = @(
    'crates/nano-model/fixtures-flux/pdf/control-request.json',
    'crates/nano-model/fixtures-flux/pdf/document-request.json',
    'crates/nano-model/fixtures-flux/pdf/document-response.json',
    'crates/nano-model/fixtures-flux/pdf/evidence-manifest.json',
    'crates/nano-model/fixtures-flux/pdf/known-quote.pdf',
    'crates/nano-model/fixtures-flux/pdf/session-transcript.json',
    'crates/nano-model/fixtures-flux/pdf/usage-summary.json'
)

function Invoke-GitText {
    param([string[]]$Arguments)
    $output = & git @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw "git $($Arguments -join ' ') failed: $output" }
    return (($output | Out-String).Trim())
}

function Invoke-GitNul {
    param([string[]]$Arguments)
    $stdout = Join-Path ([IO.Path]::GetTempPath()) ('wp03-git-' + [guid]::NewGuid().ToString('N'))
    $stderr = "$stdout.err"
    try {
        $p = Start-Process -FilePath 'git.exe' -ArgumentList $Arguments -NoNewWindow -Wait -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        if ($p.ExitCode -ne 0) { throw "git $($Arguments -join ' ') failed: $([IO.File]::ReadAllText($stderr))" }
        $text = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($stdout))
        return @([regex]::Split($text, "\x00") | Where-Object { $_.Length -gt 0 })
    } finally {
        Remove-Item -LiteralPath $stdout, $stderr -Force -ErrorAction SilentlyContinue
    }
}

function Normalize-RepoPath {
    param([string]$Path)
    $p = $Path.Replace('\', '/').Trim()
    while ($p.StartsWith('./')) { $p = $p.Substring(2) }
    return $p
}

function Get-PlanMetadata {
    param([string]$PhaseRoot)
    $plans = @(Get-ChildItem -LiteralPath $PhaseRoot -Filter '03-??-PLAN.md' -File -ErrorAction Stop | Sort-Object Name)
    $ids = @($plans | ForEach-Object { if ($_.Name -notmatch '^(03-\d{2})-PLAN\.md$') { throw "invalid plan name: $($_.Name)" }; $Matches[1] })
    $expected = @(1..13 | ForEach-Object { '03-{0:D2}' -f $_ })
    if ($plans.Count -ne 13 -or (Compare-Object $expected $ids)) { throw 'plan IDs must be exactly 03-01 through 03-13' }

    $repoPaths = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    foreach ($plan in $plans) {
        $lines = [IO.File]::ReadAllLines($plan.FullName)
        $inFiles = $false
        foreach ($line in $lines) {
            if ($line -match '^files_modified:\s*\[(.*)\]\s*$') {
                foreach ($value in ($Matches[1] -split ',')) {
                    $v = $value.Trim().Trim("'`"")
                    if ($v -and -not [IO.Path]::IsPathRooted($v)) { [void]$repoPaths.Add((Normalize-RepoPath $v)) }
                }
                break
            }
            if ($line -match '^files_modified:\s*$') { $inFiles = $true; continue }
            if ($inFiles -and $line -match '^  -\s+(.+?)\s*$') {
                $v = $Matches[1].Trim().Trim("'`"")
                if (-not [IO.Path]::IsPathRooted($v)) { [void]$repoPaths.Add((Normalize-RepoPath $v)) }
                continue
            }
            if ($inFiles -and $line -notmatch '^\s') { break }
        }
    }

    $phaseArtifacts = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    foreach ($id in $expected) {
        foreach ($suffix in @('PLAN.md', 'SUMMARY.md')) { [void]$phaseArtifacts.Add("$id-$suffix") }
    }
    foreach ($name in @('03-RESEARCH.md', '03-VALIDATION.md', 'SOURCE-AUDIT.md', '03-OWNERSHIP-PREFLIGHT.ps1', '03-CONTROL.json', '03-AUDIT.json')) {
        [void]$phaseArtifacts.Add($name)
    }
    foreach ($artifact in $phaseArtifacts) { [void]$repoPaths.Add("$PhaseRelative/$artifact") }
    [void]$repoPaths.Add('.planning/ROADMAP.md')

    $required = @(
        'crates/nano-model/data/providerCatalog.vendored.json',
        'crates/nano-model/tests/provider_catalog.rs',
        'crates/nano-model/tests/golden/provider_catalog.golden.rs',
        'UPSTREAM.md', 'docs/FOLLOWUPS.md', '.planning/ROADMAP.md'
    ) + $RequiredEvidence
    foreach ($path in $required) { if (-not $repoPaths.Contains($path)) { throw "required OWNS entry omitted: $path" } }
    foreach ($path in $repoPaths) {
        if ($path -match '[*?\[\]]' -or $path.EndsWith('/')) { throw "non-literal OWNS entry: $path" }
        if ($path -match '(^|/)\.\.(/|$)') { throw "parent traversal in OWNS entry: $path" }
    }
    return [pscustomobject]@{ PlanIds = $expected; PhaseArtifacts = @($phaseArtifacts | Sort-Object); RepoPaths = @($repoPaths | Sort-Object) }
}

function Get-ChangedPaths {
    $paths = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    $status = @(Invoke-GitNul @('status', '--porcelain=v1', '-z', '--untracked-files=all'))
    for ($i = 0; $i -lt $status.Count; $i++) {
        $record = $status[$i]
        if ($record.Length -lt 4) { throw "malformed NUL-delimited git status record (length $($record.Length), codepoints $([string]::Join(',', @($record.ToCharArray() | ForEach-Object { [int]$_ }))))" }
        $code = $record.Substring(0, 2)
        [void]$paths.Add((Normalize-RepoPath $record.Substring(3)))
        if ($code.Contains('R') -or $code.Contains('C')) {
            $i++; if ($i -ge $status.Count) { throw 'missing rename/copy source record' }
            [void]$paths.Add((Normalize-RepoPath $status[$i]))
        }
    }
    foreach ($path in (Invoke-GitNul @('diff', '--name-only', '-z', $Baseline, '--'))) { [void]$paths.Add((Normalize-RepoPath $path)) }
    $nameStatus = @(Invoke-GitNul @('diff', '--name-status', '-z', '-M', '-C', $Baseline, '--'))
    for ($i = 0; $i -lt $nameStatus.Count; $i++) {
        $code = $nameStatus[$i]
        if ($code -match '^[RC]\d*$') {
            if ($i + 2 -ge $nameStatus.Count) { throw 'malformed rename/copy diff record' }
            [void]$paths.Add((Normalize-RepoPath $nameStatus[++$i])); [void]$paths.Add((Normalize-RepoPath $nameStatus[++$i]))
        } else {
            if ($i + 1 -ge $nameStatus.Count) { throw 'malformed diff record' }
            [void]$paths.Add((Normalize-RepoPath $nameStatus[++$i]))
        }
    }
    return @($paths | Sort-Object)
}

function Assert-SafeChangedPaths {
    param([string]$Root, [string[]]$Changed, [string[]]$Allowed)
    $allowedSet = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    foreach ($path in $Allowed) { [void]$allowedSet.Add($path) }
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    foreach ($path in $Changed) {
        if (-not $allowedSet.Contains($path)) { throw "changed path is outside exact OWNS allowlist: $path" }
        $leaf = [IO.Path]::GetFullPath((Join-Path $rootFull $path.Replace('/', '\')))
        if (-not ($leaf -eq $rootFull -or $leaf.StartsWith($rootFull + '\', [StringComparison]::OrdinalIgnoreCase))) { throw "worktree escape: $path" }
        $relativeParts = $path.Split('/')
        $cursor = $rootFull
        foreach ($part in $relativeParts) {
            $cursor = Join-Path $cursor $part
            $item = Get-Item -LiteralPath $cursor -Force -ErrorAction Stop
            if ($null -ne $item.LinkType -or (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) { throw "link/reparse path rejected: $cursor" }
        }
        $resolved = (Resolve-Path -LiteralPath $leaf -ErrorAction Stop).Path
        if (-not ($resolved -eq $rootFull -or $resolved.StartsWith($rootFull + '\', [StringComparison]::OrdinalIgnoreCase))) { throw "resolved path escaped worktree: $path" }
    }
}

function Get-TreeManifest {
    param([string]$Root)
    $rootFull = (Resolve-Path -LiteralPath $Root -ErrorAction Stop).Path.TrimEnd('\')
    $rows = New-Object System.Collections.ArrayList
    $files = New-Object System.Collections.ArrayList
    $items = @(Get-ChildItem -LiteralPath $rootFull -Force -Recurse -ErrorAction Stop)
    foreach ($item in $items) {
        $relative = $item.FullName.Substring($rootFull.Length).TrimStart('\').Replace('\', '/')
        $isReparse = (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)
        if ($isReparse) {
            [void]$rows.Add([pscustomobject][ordered]@{ path = $relative; type = 'reparse'; sha256 = $null; bytes = [int64]0 })
        } elseif ($item.PSIsContainer) {
            [void]$rows.Add([pscustomobject][ordered]@{ path = $relative; type = 'directory'; sha256 = $null; bytes = [int64]0 })
        } else {
            [void]$files.Add($item.FullName)
        }
    }

    $workerCount = [Math]::Min(8, [Math]::Max(1, $files.Count))
    $chunks = @(); for ($i = 0; $i -lt $workerCount; $i++) { $chunks += ,(New-Object System.Collections.ArrayList) }
    for ($i = 0; $i -lt $files.Count; $i++) { [void]$chunks[$i % $workerCount].Add($files[$i]) }
    $pool = [RunspaceFactory]::CreateRunspacePool(1, $workerCount)
    $pool.Open()
    $jobs = New-Object System.Collections.ArrayList
    $worker = {
        param($WorkerRoot, $WorkerFiles)
        $sha = [Security.Cryptography.SHA256]::Create()
        try {
            foreach ($fullName in $WorkerFiles) {
                $info = Get-Item -LiteralPath $fullName -Force -ErrorAction Stop
                $stream = [IO.File]::Open($fullName, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
                try { $hash = ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() } finally { $stream.Dispose() }
                [pscustomobject][ordered]@{
                    path = $fullName.Substring($WorkerRoot.Length).TrimStart('\').Replace('\', '/')
                    type = 'file'; sha256 = $hash; bytes = [int64]$info.Length
                }
            }
        } finally { $sha.Dispose() }
    }
    try {
        foreach ($chunk in $chunks) {
            $ps = [PowerShell]::Create()
            $ps.RunspacePool = $pool
            [void]$ps.AddScript($worker).AddArgument($rootFull).AddArgument(@($chunk))
            [void]$jobs.Add([pscustomobject]@{ PowerShell = $ps; Handle = $ps.BeginInvoke() })
        }
        foreach ($job in $jobs) {
            foreach ($row in $job.PowerShell.EndInvoke($job.Handle)) { [void]$rows.Add($row) }
            if ($job.PowerShell.HadErrors) { throw ($job.PowerShell.Streams.Error | Out-String) }
        }
    } finally {
        foreach ($job in $jobs) { $job.PowerShell.Dispose() }
        $pool.Close(); $pool.Dispose()
    }
    return @($rows | Sort-Object { $_.path })
}

function Write-JsonUtf8 {
    param([string]$Path, [object]$Value, [int]$Depth = 12)
    [IO.File]::WriteAllText($Path, (($Value | ConvertTo-Json -Depth $Depth) + "`n"), (New-Object Text.UTF8Encoding($false)))
}

function Read-Manifest {
    param([object]$Entry)
    if (-not (Test-Path -LiteralPath $Entry.path -PathType Leaf)) { throw "missing durable manifest: $($Entry.path)" }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Entry.path -ErrorAction Stop).Hash.ToLowerInvariant()
    if ($hash -ne $Entry.sha256) { throw "durable manifest hash mismatch: $($Entry.path)" }
    return @((Get-Content -Raw -LiteralPath $Entry.path -ErrorAction Stop | ConvertFrom-Json))
}

function Compare-ManifestExact {
    param([object[]]$Before, [object[]]$After, [string]$Name)
    $a = @($Before | ForEach-Object { "$($_.path)|$($_.type)|$($_.sha256)|$($_.bytes)" })
    $b = @($After | ForEach-Object { "$($_.path)|$($_.type)|$($_.sha256)|$($_.bytes)" })
    if (Compare-Object $a $b) { throw "$Name external tree changed" }
}

function Assert-SharedDelta {
    param([object[]]$Before, [object[]]$After, [string]$RepoRoot)
    $allowed = @('contracts/nano-error-codes.json') + @($RequiredEvidence | ForEach-Object { $_.Substring('crates/nano-model/'.Length) }) + @('fixtures/flux/pdf/canary-receipt.json')
    $old = @{}; foreach ($row in $Before) { $old[$row.path] = "$($row.type)|$($row.sha256)|$($row.bytes)" }
    $new = @{}; foreach ($row in $After) { $new[$row.path] = "$($row.type)|$($row.sha256)|$($row.bytes)" }
    $all = @($old.Keys + $new.Keys | Sort-Object -Unique)
    foreach ($path in $all) {
        if ($old[$path] -ne $new[$path] -and $path -notin $allowed) { throw "undeclared shared delta: $path" }
    }
    $pairs = @(@{ shared = 'contracts/nano-error-codes.json'; repo = 'crates/nano-session/contracts/nano-error-codes.json' })
    foreach ($evidence in $RequiredEvidence) { $pairs += @{ shared = $evidence.Substring('crates/nano-model/'.Length); repo = $evidence } }
    foreach ($pair in $pairs) {
        $sharedPath = Join-Path $SharedRoot $pair.shared.Replace('/', '\')
        $repoPath = Join-Path $RepoRoot $pair.repo.Replace('/', '\')
        if ((Test-Path -LiteralPath $sharedPath -PathType Leaf) -xor (Test-Path -LiteralPath $repoPath -PathType Leaf)) { throw "paired file exists on only one side: $($pair.repo)" }
        if (Test-Path -LiteralPath $sharedPath -PathType Leaf) {
            $sh = (Get-FileHash -Algorithm SHA256 -LiteralPath $sharedPath -ErrorAction Stop).Hash
            $rh = (Get-FileHash -Algorithm SHA256 -LiteralPath $repoPath -ErrorAction Stop).Hash
            if ($sh -ne $rh) { throw "paired file hash mismatch: $($pair.repo)" }
        }
    }
}

$RepoRoot = (Resolve-Path -LiteralPath (Invoke-GitText @('rev-parse', '--show-toplevel')) -ErrorAction Stop).Path
$Branch = Invoke-GitText @('rev-parse', '--abbrev-ref', 'HEAD')
if ($Branch -ne 'feat/wp-03') { throw "unexpected branch: $Branch" }
if ((Invoke-GitText @('cat-file', '-t', $Baseline)) -ne 'commit') { throw 'locked baseline is unavailable' }
$PhaseRoot = Join-Path $RepoRoot $PhaseRelative.Replace('/', '\')
$metadata = Get-PlanMetadata $PhaseRoot
$changed = Get-ChangedPaths
Assert-SafeChangedPaths $RepoRoot $changed $metadata.RepoPaths

$MonorepoRoot = Split-Path -Parent (Split-Path -Parent $RepoRoot)
$NanoRoot = Join-Path $MonorepoRoot 'nano'
$ResourcesRoot = Join-Path $MonorepoRoot 'resources\upstreams'
$SharedRoot = Join-Path $MonorepoRoot 'shared'
$ControlPath = Join-Path $RepoRoot $ControlRelative.Replace('/', '\')

if ($Mode -eq 'Initialize') {
    if (Test-Path -LiteralPath $ControlPath) { throw 'control state already exists; Initialize never overwrites a baseline' }
    $durable = Join-Path ([IO.Path]::GetTempPath()) ('wayland-nano-wp03-' + [guid]::NewGuid().ToString('N'))
    [void](New-Item -ItemType Directory -Path $durable -ErrorAction Stop)
    $manifestEntries = [ordered]@{}
    foreach ($name in @('nano', 'resources_upstreams', 'shared')) {
        $root = if ($name -eq 'nano') { $NanoRoot } elseif ($name -eq 'resources_upstreams') { $ResourcesRoot } else { $SharedRoot }
        $manifestPath = Join-Path $durable "$name-manifest.json"
        Write-JsonUtf8 $manifestPath (Get-TreeManifest $root)
        $manifestEntries[$name] = [ordered]@{ path = $manifestPath; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $manifestPath -ErrorAction Stop).Hash.ToLowerInvariant() }
    }
    $control = [ordered]@{
        schema = 'wp03_control_v1'; baseline = $Baseline; branch = $Branch; worktree = $RepoRoot
        initialized_at = [DateTime]::UtcNow.ToString('o'); plan_ids = $metadata.PlanIds
        phase_artifacts = $metadata.PhaseArtifacts; repo_allowlist = $metadata.RepoPaths
        shared_allowlist = @('contracts/nano-error-codes.json') + @($RequiredEvidence | ForEach-Object { $_.Substring('crates/nano-model/'.Length) }) + @('fixtures/flux/pdf/canary-receipt.json')
        manifests = $manifestEntries
    }
    Write-JsonUtf8 $ControlPath $control
    Write-Output "WP-0.3 ownership control initialized at $durable"
    exit 0
}

$control = Get-Content -Raw -LiteralPath $ControlPath -ErrorAction Stop | ConvertFrom-Json
if ($control.schema -ne 'wp03_control_v1' -or $control.baseline -ne $Baseline -or $control.branch -ne $Branch -or $control.worktree -ne $RepoRoot) { throw 'control identity mismatch' }
if (Compare-Object $metadata.PlanIds @($control.plan_ids)) { throw 'control plan IDs drifted' }
if (Compare-Object $metadata.PhaseArtifacts @($control.phase_artifacts)) { throw 'control phase artifacts drifted' }
if (Compare-Object $metadata.RepoPaths @($control.repo_allowlist)) { throw 'control OWNS allowlist drifted' }

$nanoBefore = Read-Manifest $control.manifests.nano
$resourcesBefore = Read-Manifest $control.manifests.resources_upstreams
$sharedBefore = Read-Manifest $control.manifests.shared
Compare-ManifestExact $nanoBefore (Get-TreeManifest $NanoRoot) 'nano'
Compare-ManifestExact $resourcesBefore (Get-TreeManifest $ResourcesRoot) 'resources/upstreams'
Assert-SharedDelta $sharedBefore (Get-TreeManifest $SharedRoot) $RepoRoot

if ($Mode -eq 'Closure') {
    $control | Add-Member -Force -NotePropertyName closure -NotePropertyValue ([ordered]@{
        status = 'PASS'; timestamp = [DateTime]::UtcNow.ToString('o'); nano_equal = $true
        resources_upstreams_equal = $true; shared_delta_valid = $true; pairs_valid = $true
    })
    Write-JsonUtf8 $ControlPath $control
}

Write-Output "WP-0.3 ownership $Mode PASS"
