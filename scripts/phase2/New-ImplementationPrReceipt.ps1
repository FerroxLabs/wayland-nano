[CmdletBinding()]
param(
    [Parameter(Mandatory)] [int] $PrNumber,
    [Parameter(Mandatory)] [string] $ExpectedHead,
    [Parameter(Mandatory)] [Int64] $ExpectedRunId,
    [string] $ReceiptPath = 'docs/evidence/phase2/nano-pr-governance.json',
    [string] $Repository = 'FerroxLabs/wayland-nano',
    [string] $ExpectedBase = 'master',
    [string] $ExpectedHeadRef = 'feat/p2-minimal-authenticated-activation',
    [string] $ExpectedAuthor = 'FerroxLabs'
)

$ErrorActionPreference = 'Stop'
$requiredChecks = @(
    'gate (windows-latest, x64)',
    'gate (windows-11-arm, arm64)',
    'gate (macos-14, arm64)',
    'gate (macos-15-intel, x64)',
    'gate (ubuntu-22.04, x64)',
    'gate (ubuntu-24.04-arm, arm64)',
    'gate-cards'
)
$requiredCodeowners = @(
    '/CODEOWNERS @FerroxLabs @TradeCanyon',
    '/gates/** @FerroxLabs @TradeCanyon',
    '/agents/** @FerroxLabs @TradeCanyon'
)

function Invoke-GhJson([string[]] $Arguments) {
    $output = & gh @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw "gh failed ($LASTEXITCODE): $($output -join [Environment]::NewLine)" }
    ($output -join [Environment]::NewLine) | ConvertFrom-Json
}
function Assert([bool] $Condition, [string] $Message) { if (-not $Condition) { throw $Message } }
function Git([string[]] $Arguments) {
    $output = & git @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw "git failed ($LASTEXITCODE): $($output -join [Environment]::NewLine)" }
    ($output -join "`n").Trim()
}
function Hash-GitBlob([string] $Object) {
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = 'git'; $psi.Arguments = "cat-file blob $Object"
    $psi.UseShellExecute = $false; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($psi)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha.ComputeHash($process.StandardOutput.BaseStream)
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "git cat-file failed: $($process.StandardError.ReadToEnd())" }
        -join ($hash | ForEach-Object { $_.ToString('x2') })
    } finally { $sha.Dispose(); $process.Dispose() }
}

Assert ($ExpectedHead -cmatch '^[0-9a-f]{40}$') 'ExpectedHead must be a lowercase 40-hex commit.'
$pr = Invoke-GhJson @('pr','view',[string]$PrNumber,'--repo',$Repository,'--json','number,url,state,baseRefName,headRefName,headRefOid,author,mergeStateStatus,reviewDecision,statusCheckRollup,reviews,mergeCommit,mergedBy,mergedAt')
Assert ($pr.number -eq $PrNumber) 'Unexpected PR number.'
Assert ($pr.baseRefName -ceq $ExpectedBase) 'Unexpected PR base.'
Assert ($pr.headRefName -ceq $ExpectedHeadRef) 'Unexpected PR head branch.'
Assert ($pr.headRefOid -ceq $ExpectedHead) 'Live PR head differs from immutable expected head.'
Assert ($pr.author.login -ceq $ExpectedAuthor) 'Unexpected PR author.'

$checks = @($pr.statusCheckRollup | Where-Object { $_.__typename -eq 'CheckRun' -and $_.name -in $requiredChecks })
Assert ($checks.Count -eq 7) 'Expected exactly seven required check runs.'
Assert (@($checks.name | Sort-Object -Unique).Count -eq 7) 'Required check names are duplicated.'
Assert (-not (Compare-Object ($requiredChecks | Sort-Object) ($checks.name | Sort-Object))) 'Required check-name set differs.'
foreach ($check in $checks) {
    Assert ($check.status -ceq 'COMPLETED' -and $check.conclusion -ceq 'SUCCESS') "Required check '$($check.name)' is not successful."
    Assert ($check.detailsUrl -match "/actions/runs/$ExpectedRunId(?:/|$)") "Required check '$($check.name)' is not bound to run $ExpectedRunId."
}
$observedRunIds = @($checks.detailsUrl | ForEach-Object { if ($_ -match '/actions/runs/(\d+)(?:/|$)') { [Int64]$Matches[1] } } | Sort-Object -Unique)
Assert ($observedRunIds.Count -eq 1 -and $observedRunIds[0] -eq $ExpectedRunId) 'The seven checks do not bind exactly one expected workflow run.'

$codeowners = Git @('show',"$ExpectedHead`:CODEOWNERS")
$rules = @($codeowners -split "`r?`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith('#') })
Assert ($rules.Count -eq 3 -and -not (Compare-Object $rules $requiredCodeowners)) 'Exact CODEOWNERS rules mismatch.'
$protection = Invoke-GhJson @('api',"repos/$Repository/branches/$ExpectedBase/protection")
$contexts = @($protection.required_status_checks.contexts)
Assert ($protection.required_status_checks.strict -and $contexts.Count -eq 7 -and -not (Compare-Object ($contexts | Sort-Object) ($requiredChecks | Sort-Object))) 'Required strict seven-check protection mismatch.'
Assert ($protection.required_pull_request_reviews.require_code_owner_reviews -and $protection.required_pull_request_reviews.required_approving_review_count -eq 1 -and $protection.required_pull_request_reviews.dismiss_stale_reviews -and $protection.required_pull_request_reviews.require_last_push_approval) 'Review protection mismatch.'
Assert ($protection.enforce_admins.enabled -and $protection.required_conversation_resolution.enabled -and -not $protection.allow_force_pushes.enabled -and -not $protection.allow_deletions.enabled) 'No-bypass branch protection mismatch.'
$rulesets = @(Invoke-GhJson @('api',"repos/$Repository/rulesets"))
Assert ($rulesets.Count -eq 0) 'Unexpected repository ruleset may add bypass behavior.'

$lockObject = "$ExpectedHead`:Cargo.lock"
$lockHash = Hash-GitBlob $lockObject
$enablementBlob = Git @('rev-parse',"$ExpectedHead`:crates/nano-activation/src/enablement.rs")
$enablementTestBlob = Git @('rev-parse',"$ExpectedHead`:crates/nano-activation/tests/enablement.rs")
$enablementText = Git @('show',"$ExpectedHead`:crates/nano-activation/src/enablement.rs")
Assert ($enablementText -match 'replay\(\)\?\.ok_or\(EnablementError::Missing\)') 'Default-off missing-state enforcement is absent.'

$exactReview = @($pr.reviews | Where-Object { $_.author.login -ceq 'TradeCanyon' -and $_.state -ceq 'APPROVED' -and $_.commit.oid -ceq $ExpectedHead } | Select-Object -Last 1)
$receipt = [ordered]@{
    schema_version = 'phase2-nano-implementation-governance-v1'
    captured_at_utc = [DateTime]::UtcNow.ToString('o')
    repository = $Repository
    governance_model = 'single-human-distinct-account'
    same_human_controller = $true
    independent_human_review = $false
    owner_directed_agent_operated_review = $true
    executor_did_switch_review_merge = $true
    pr_number = $PrNumber; url = $pr.url; state = $pr.state
    base_ref = $pr.baseRefName; head_ref = $pr.headRefName; author_login = $pr.author.login
    head_sha = $pr.headRefOid; expected_run_id = $ExpectedRunId
    checks = @($checks | Sort-Object name | ForEach-Object { [ordered]@{ name=$_.name; status=$_.status; conclusion=$_.conclusion; details_url=$_.detailsUrl; run_id=$ExpectedRunId } })
    codeowners = [ordered]@{ blob_sha=(Git @('rev-parse',"$ExpectedHead`:CODEOWNERS")); rules=$rules }
    protection = [ordered]@{ strict=$true; required_status_checks=7; required_codeowner_approvals=1; dismiss_stale_reviews=$true; require_last_push_approval=$true; require_conversation_resolution=$true; enforce_admins=$true; allow_force_pushes=$false; allow_deletions=$false; bypass_actors=@() }
    review = if ($exactReview.Count) { [ordered]@{ id=$exactReview[0].id; state=$exactReview[0].state; reviewer_login=$exactReview[0].author.login; commit_oid=$exactReview[0].commit.oid } } else { $null }
    merger_login = if ($pr.mergedBy) { $pr.mergedBy.login } else { $null }
    merge_commit_sha = if ($pr.mergeCommit) { $pr.mergeCommit.oid } else { $null }
    merged_at = $pr.mergedAt
    artifact = [ordered]@{ source_commit_sha=$ExpectedHead; cargo_lock_sha256=$lockHash; cargo_lock_blob_sha=(Git @('rev-parse',"$ExpectedHead`:Cargo.lock")) }
    default_off = [ordered]@{ required=$true; missing_state_refuses=$true; implementation_blob_sha=$enablementBlob; test_blob_sha=$enablementTestBlob }
}
$parent = Split-Path -Parent $ReceiptPath
if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
$utf8 = New-Object Text.UTF8Encoding($false)
[IO.File]::WriteAllText((Join-Path (Get-Location) $ReceiptPath), ($receipt | ConvertTo-Json -Depth 10), $utf8)
Write-Output "PASS: captured exact Phase 2 Nano PR #$PrNumber head $ExpectedHead and run $ExpectedRunId"
