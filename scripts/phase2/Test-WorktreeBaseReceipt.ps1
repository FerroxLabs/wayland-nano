[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ReceiptPath,
    [Parameter(Mandatory = $true)][ValidateSet('Nano', 'Desktop')][string]$Kind,
    [Parameter(Mandatory = $true)][string]$ExpectedTarget,
    [Parameter(Mandatory = $true)][string]$ExpectedBranch,
    [Parameter(Mandatory = $true)][string]$ExpectedRemote,
    [switch]$RequirePr9,
    [switch]$RequireExactSevenChecks,
    [switch]$RequireBaseEqualsRemote,
    [switch]$RequirePrimaryUntouched,
    [switch]$RequireAuthorizedRemoteBase,
    [switch]$RequireLiveAreaDesktopIssue,
    [switch]$RequireWlQueue
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-True([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Get-CanonicalPath([string]$Path) { [IO.Path]::GetFullPath($Path).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar).Replace('\', '/') }
function Get-Sha256Text([string]$Text) {
    $bytes = [Text.Encoding]::UTF8.GetBytes($Text); $sha = [Security.Cryptography.SHA256]::Create()
    try { ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant() } finally { $sha.Dispose() }
}
function Invoke-Git([string]$Root, [Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments) {
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $output = & git -C $Root @Arguments 2>&1 }
    finally { $ErrorActionPreference = $previousPreference }
    if ($LASTEXITCODE -ne 0) { throw "git $($Arguments -join ' ') failed: $($output -join [Environment]::NewLine)" }
    ($output -join "`n").Trim()
}

Assert-True (Test-Path -LiteralPath $ReceiptPath -PathType Leaf) "Receipt missing: $ReceiptPath"
$receipt = Get-Content -Raw -LiteralPath $ReceiptPath | ConvertFrom-Json
Assert-True ($receipt.schema -eq 'wayland.nano.phase2.worktree-base/v1') 'Unexpected receipt schema'
Assert-True ($receipt.kind -eq $Kind) 'Receipt kind mismatch'

$target = Get-CanonicalPath $ExpectedTarget
$root = Get-CanonicalPath $receipt.repository_root
Assert-True ((Get-CanonicalPath $receipt.target_path) -eq $target) 'Target path mismatch'
Assert-True ($target.StartsWith("$root/", [StringComparison]::OrdinalIgnoreCase)) 'Target escapes repository root'
Assert-True ((Invoke-Git $root remote get-url origin).TrimEnd('/') -eq $ExpectedRemote.TrimEnd('/')) 'Remote URL mismatch'
Assert-True ((Invoke-Git $target branch --show-current) -eq $ExpectedBranch) 'Worktree branch mismatch'
Assert-True ((Invoke-Git $target rev-parse HEAD) -eq $receipt.base_sha) 'Worktree HEAD differs from receipt base'
Assert-True (-not (Invoke-Git $target status --porcelain=v1 --untracked-files=all)) 'Worktree is not clean'

$commonRaw = Invoke-Git $root rev-parse --git-common-dir
$common = if ([IO.Path]::IsPathRooted($commonRaw)) { Get-CanonicalPath $commonRaw } else { Get-CanonicalPath (Join-Path $root $commonRaw) }
$targetCommonRaw = Invoke-Git $target rev-parse --git-common-dir
$targetCommon = if ([IO.Path]::IsPathRooted($targetCommonRaw)) { Get-CanonicalPath $targetCommonRaw } else { Get-CanonicalPath (Join-Path $target $targetCommonRaw) }
Assert-True ($common -eq $targetCommon -and $common -eq (Get-CanonicalPath $receipt.git_common_dir)) 'Git common directory mismatch'

$porcelain = Invoke-Git $root worktree list --porcelain
$pathMatches = @([regex]::Matches($porcelain, "(?m)^worktree $([regex]::Escape($target))$"))
$branchMatches = @([regex]::Matches($porcelain, "(?m)^branch refs/heads/$([regex]::Escape($ExpectedBranch))$"))
Assert-True ($pathMatches.Count -eq 1) 'Target worktree path is not unique'
Assert-True ($branchMatches.Count -eq 1) 'Target worktree branch is not unique'

if ($RequireBaseEqualsRemote -or $RequireAuthorizedRemoteBase) {
    $baseBranch = $receipt.base_ref -replace '^origin/', ''
    Invoke-Git $root fetch origin $baseBranch | Out-Null
    Assert-True ((Invoke-Git $root rev-parse "origin/$baseBranch^{commit}") -eq $receipt.base_sha) 'Receipt base is not the freshly fetched remote base'
}

if ($RequirePrimaryUntouched) {
    Assert-True ($receipt.primary.head_before -eq $receipt.primary.head_after) 'Receipt records changed primary HEAD'
    Assert-True ($receipt.primary.branch_before -eq $receipt.primary.branch_after) 'Receipt records changed primary branch'
    Assert-True ($receipt.primary.status_sha256_before -eq $receipt.primary.status_sha256_after) 'Receipt records changed primary status'
    Assert-True ((Invoke-Git $root rev-parse HEAD) -eq $receipt.primary.head_after) 'Primary HEAD changed since receipt creation'
    Assert-True ((Invoke-Git $root branch --show-current) -eq $receipt.primary.branch_after) 'Primary branch changed since receipt creation'
    $targetRelative = $target.Substring(("$root/").Length)
    $currentStatus = Invoke-Git $root status --porcelain=v1 --untracked-files=normal -- . ":(exclude)$targetRelative"
    Assert-True ((Get-Sha256Text $currentStatus) -eq $receipt.primary.status_sha256_after) 'Primary status changed since receipt creation'
}

if ($RequirePr9) {
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $raw = & gh pr view 9 -R FerroxLabs/wayland-nano --json number,state,baseRefName,headRefName,headRefOid,mergeCommit,author,reviews,statusCheckRollup,url 2>&1 }
    finally { $ErrorActionPreference = $previousPreference }
    if ($LASTEXITCODE -ne 0) { throw "Unable to query PR #9: $($raw -join [Environment]::NewLine)" }
    $pr = ($raw -join "`n") | ConvertFrom-Json
    Assert-True ($pr.state -eq 'MERGED' -and $pr.baseRefName -eq 'master') 'PR #9 is not merged to master'
    Assert-True ($pr.mergeCommit.oid -eq $receipt.base_sha) 'PR #9 merge SHA differs from worktree base'
    & git -C $root merge-base --is-ancestor $pr.headRefOid $receipt.base_sha
    Assert-True ($LASTEXITCODE -eq 0) 'PR #9 head is not an ancestor of merge/base'
    $approved = @($pr.reviews | Where-Object { $_.state -eq 'APPROVED' -and $_.commit.oid -eq $pr.headRefOid })
    Assert-True ($approved.Count -ge 1) 'PR #9 has no approval bound to exact head'

    if ($RequireExactSevenChecks) {
        $expected = @('gate (windows-latest, x64)', 'gate (windows-11-arm, arm64)', 'gate (macos-14, arm64)', 'gate (macos-15-intel, x64)', 'gate (ubuntu-22.04, x64)', 'gate (ubuntu-24.04-arm, arm64)', 'gate-cards')
        $actual = @($pr.statusCheckRollup | ForEach-Object { $_.name })
        Assert-True ($actual.Count -eq 7) "Expected exactly seven checks, got $($actual.Count)"
        Assert-True (@($actual | Sort-Object -Unique).Count -eq 7) 'Check names are not unique'
        Assert-True (-not (Compare-Object ($expected | Sort-Object) ($actual | Sort-Object))) 'Check-name set differs from required seven checks'
        foreach ($check in $pr.statusCheckRollup) {
            Assert-True ($check.status -eq 'COMPLETED' -and $check.conclusion -eq 'SUCCESS') "Check is not successful: $($check.name)"
            Assert-True (-not [string]::IsNullOrWhiteSpace($check.detailsUrl)) "Check lacks details URL: $($check.name)"
        }
    }
}

if ($Kind -eq 'Desktop') {
    Assert-True ($env:WL_LANE -eq 'desktop') 'WL_LANE must equal desktop'
    Assert-True ($receipt.coordination.mechanism -eq 'owner-directed authenticated GitHub board fallback') 'Desktop coordination fallback is not explicit'
    Assert-True ($receipt.coordination.wl_available -eq $false -and $receipt.coordination.wl_attempts -eq 2) 'Missing wl deviation is not accurately recorded'
    Assert-True (-not [string]::IsNullOrWhiteSpace($receipt.coordination.owner_authorization)) 'Owner authorization text missing'
    Assert-True ((Get-Sha256Text $receipt.coordination.owner_authorization) -eq $receipt.coordination.owner_authorization_sha256) 'Owner authorization hash mismatch'

    if ($RequireWlQueue) {
        Assert-True ($receipt.coordination.wl_deviation -match 'proven absent') 'Required wl absence/deviation evidence missing'
    }
    if ($RequireLiveAreaDesktopIssue) {
        $number = [int]$receipt.desktop_issue.number
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try { $raw = & gh issue view $number -R FerroxLabs/wayland --json number,state,title,assignees,labels,url,author 2>&1 }
        finally { $ErrorActionPreference = $previousPreference }
        if ($LASTEXITCODE -ne 0) { throw "Unable to query Desktop issue: $($raw -join [Environment]::NewLine)" }
        $issue = ($raw -join "`n") | ConvertFrom-Json
        Assert-True ($issue.number -eq $number -and $issue.state -eq 'OPEN') 'Desktop issue is not live and open'
        Assert-True (@($issue.assignees.login) -contains 'FerroxLabs') 'Desktop issue is not assigned to FerroxLabs'
        $labels = @($issue.labels.name)
        Assert-True ($labels -contains 'area:desktop-ui') 'Desktop issue lacks area:desktop-ui'
        Assert-True ($labels -contains 'needs:desktop') 'Desktop issue lacks needs:desktop'
        Assert-True ($labels -contains 'state:in-progress') 'Desktop issue lacks state:in-progress'
    }
}

Write-Output "PASS: $Kind worktree receipt is live-verifiable and exact ($($receipt.base_sha))"
