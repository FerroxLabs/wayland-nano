[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Nano', 'Desktop')]
    [string]$Kind,

    [Parameter(Mandatory = $true)]
    [string]$RepositoryRoot,

    [Parameter(Mandatory = $true)]
    [string]$TargetPath,

    [Parameter(Mandatory = $true)]
    [string]$Branch,

    [Parameter(Mandatory = $true)]
    [string]$BaseRef,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedRemote,

    [Parameter(Mandatory = $true)]
    [string]$ReceiptPath,

    [int]$IssueNumber = 0
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Git {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $output = & git -C $RepositoryRoot @Arguments 2>&1 }
    finally { $ErrorActionPreference = $previousPreference }
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed: $($output -join [Environment]::NewLine)"
    }
    return ($output -join "`n").Trim()
}

function Get-Sha256Text([string]$Text) {
    $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Get-CanonicalPath([string]$Path) {
    return [IO.Path]::GetFullPath($Path).TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar).Replace('\', '/')
}

$repository = Get-CanonicalPath $RepositoryRoot
$target = Get-CanonicalPath $TargetPath
$allowedRoot = "$repository/"
if (-not $target.StartsWith($allowedRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Target path is outside repository root: $target"
}
$targetRelative = $target.Substring($allowedRoot.Length)

function Get-PrimaryStatus {
    # The linked worktree is intentionally nested below the primary checkout. Exclude
    # only that exact authorized path while retaining all pre-existing dirty entries.
    return Invoke-Git status --porcelain=v1 --untracked-files=normal -- . ":(exclude)$targetRelative"
}

$remoteUrl = Invoke-Git remote get-url origin
if ($remoteUrl.TrimEnd('/') -ne $ExpectedRemote.TrimEnd('/')) {
    throw "Unexpected origin URL: $remoteUrl"
}

$commonDirRaw = Invoke-Git rev-parse --git-common-dir
$commonDir = if ([IO.Path]::IsPathRooted($commonDirRaw)) { Get-CanonicalPath $commonDirRaw } else { Get-CanonicalPath (Join-Path $repository $commonDirRaw) }
$primaryHeadBefore = Invoke-Git rev-parse HEAD
$primaryBranchBefore = Invoke-Git branch --show-current
$primaryStatusBefore = Get-PrimaryStatus
$primaryStatusHashBefore = Get-Sha256Text $primaryStatusBefore
$worktreesBefore = Invoke-Git worktree list --porcelain

$targetExists = Test-Path -LiteralPath $target
$branchExists = [bool]((Invoke-Git branch --list $Branch).Trim())
$targetRegistered = $worktreesBefore -match "(?m)^worktree $([regex]::Escape($target))$"
if (($targetExists -or $branchExists -or $targetRegistered) -and -not ($targetExists -and $branchExists -and $targetRegistered)) {
    throw 'Target path, branch, and worktree registration exist only partially'
}

Invoke-Git fetch origin $BaseRef | Out-Null
$baseSha = Invoke-Git rev-parse "origin/$BaseRef^{commit}"

$pr9 = $null
$checks = @()
if ($Kind -eq 'Nano') {
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $pr9Raw = & gh pr view 9 -R FerroxLabs/wayland-nano --json number,state,baseRefName,headRefName,headRefOid,mergeCommit,author,reviews,statusCheckRollup,url 2>&1 }
    finally { $ErrorActionPreference = $previousPreference }
    if ($LASTEXITCODE -ne 0) { throw "Unable to query PR #9: $($pr9Raw -join [Environment]::NewLine)" }
    $pr9 = ($pr9Raw -join "`n") | ConvertFrom-Json
    if ($pr9.state -ne 'MERGED' -or $pr9.baseRefName -ne 'master') { throw 'PR #9 is not merged to master' }
    if ($pr9.mergeCommit.oid -ne $baseSha) { throw "origin/master $baseSha is not PR #9 merge $($pr9.mergeCommit.oid)" }
    & git -C $RepositoryRoot merge-base --is-ancestor $pr9.headRefOid $baseSha
    if ($LASTEXITCODE -ne 0) { throw 'PR #9 reviewed head is not an ancestor of the merge/base SHA' }
    $checks = @($pr9.statusCheckRollup | ForEach-Object {
        [ordered]@{ name = $_.name; status = $_.status; conclusion = $_.conclusion; details_url = $_.detailsUrl }
    })
}

$issue = $null
$coordination = $null
if ($Kind -eq 'Desktop') {
    if ($IssueNumber -le 0) { throw 'Desktop receipt requires -IssueNumber' }
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $issueRaw = & gh issue view $IssueNumber -R FerroxLabs/wayland --json number,state,title,assignees,labels,url,author,body 2>&1 }
    finally { $ErrorActionPreference = $previousPreference }
    if ($LASTEXITCODE -ne 0) { throw "Unable to query Desktop issue: $($issueRaw -join [Environment]::NewLine)" }
    $issue = ($issueRaw -join "`n") | ConvertFrom-Json
    $coordination = [ordered]@{
        lane = 'desktop'
        mechanism = 'owner-directed authenticated GitHub board fallback'
        issue_number = $IssueNumber
        issue_output_sha256 = Get-Sha256Text ($issueRaw -join "`n")
        wl_available = $false
        wl_attempts = 2
        wl_deviation = 'wl wrapper proven absent in PowerShell, Git Bash, common roots, and public search; owner authorized live GitHub issue #1201 as coordination authority; wl was not retried or fabricated'
        owner_authorization = "owner-authorized remote origin/$BaseRef at freshly fetched exact SHA; issue #$IssueNumber"
        owner_authorization_sha256 = Get-Sha256Text "owner-authorized remote origin/$BaseRef at freshly fetched exact SHA; issue #$IssueNumber"
    }
}

if (-not $targetExists) {
    Invoke-Git worktree add -b $Branch $target $baseSha | Out-Null
}

$targetHead = (& git -C $target rev-parse HEAD 2>&1) -join "`n"
if ($LASTEXITCODE -ne 0) { throw "Unable to inspect target head: $targetHead" }
$targetBranch = ((& git -C $target branch --show-current 2>&1) -join "`n").Trim()
$targetStatus = ((& git -C $target status --porcelain=v1 --untracked-files=all 2>&1) -join "`n").Trim()
$targetCommonRaw = ((& git -C $target rev-parse --git-common-dir 2>&1) -join "`n").Trim()
$targetCommon = if ([IO.Path]::IsPathRooted($targetCommonRaw)) { Get-CanonicalPath $targetCommonRaw } else { Get-CanonicalPath (Join-Path $target $targetCommonRaw) }

$primaryHeadAfter = Invoke-Git rev-parse HEAD
$primaryBranchAfter = Invoke-Git branch --show-current
$primaryStatusAfter = Get-PrimaryStatus
$primaryStatusHashAfter = Get-Sha256Text $primaryStatusAfter

if ($primaryHeadBefore -ne $primaryHeadAfter -or $primaryBranchBefore -ne $primaryBranchAfter -or $primaryStatusHashBefore -ne $primaryStatusHashAfter) {
    throw 'Primary checkout HEAD, branch, or status changed while creating the worktree'
}
if ($targetHead.Trim() -ne $baseSha -or $targetBranch -ne $Branch -or $targetStatus) { throw 'Created worktree identity or cleanliness is wrong' }
if ($targetCommon -ne $commonDir) { throw 'Created worktree does not share the expected git common directory' }

$receipt = [ordered]@{
    schema = 'wayland.nano.phase2.worktree-base/v1'
    generated_at_utc = [DateTime]::UtcNow.ToString('o')
    kind = $Kind
    repository_root = $repository
    remote_url = $remoteUrl
    git_common_dir = $commonDir
    target_path = $target
    branch = $Branch
    base_ref = "origin/$BaseRef"
    base_sha = $baseSha
    target_head = $targetHead.Trim()
    target_clean = $true
    exact_reuse = [bool]$targetExists
    primary = [ordered]@{
        path = $repository
        branch_before = $primaryBranchBefore
        branch_after = $primaryBranchAfter
        head_before = $primaryHeadBefore
        head_after = $primaryHeadAfter
        status_sha256_before = $primaryStatusHashBefore
        status_sha256_after = $primaryStatusHashAfter
        status_mode = "porcelain-v1/untracked-normal/exclude:$targetRelative"
        untouched = $true
    }
    pr9 = if ($pr9) { [ordered]@{
        number = $pr9.number; state = $pr9.state; base_ref = $pr9.baseRefName; head_ref = $pr9.headRefName
        head_sha = $pr9.headRefOid; merge_sha = $pr9.mergeCommit.oid; author = $pr9.author.login
        approved_head_reviews = @($pr9.reviews | Where-Object { $_.state -eq 'APPROVED' -and $_.commit.oid -eq $pr9.headRefOid } | ForEach-Object { [ordered]@{ author = $_.author.login; commit = $_.commit.oid; submitted_at = $_.submittedAt } })
        checks = $checks; url = $pr9.url
    } } else { $null }
    desktop_issue = if ($issue) { [ordered]@{
        repository = 'FerroxLabs/wayland'; number = $issue.number; state = $issue.state; title = $issue.title
        author = $issue.author.login; assignees = @($issue.assignees.login); labels = @($issue.labels.name); url = $issue.url
    } } else { $null }
    coordination = $coordination
}

$receiptDirectory = Split-Path -Parent $ReceiptPath
if ($receiptDirectory) { New-Item -ItemType Directory -Path $receiptDirectory -Force | Out-Null }
$receipt | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $ReceiptPath -Encoding UTF8
Write-Output "Created and receipted $Kind worktree at $target ($baseSha)"
