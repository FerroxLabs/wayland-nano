[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $ReceiptPath,
    [Parameter(Mandatory)] [string] $ExpectedState,
    [Parameter(Mandatory)] [string] $ExpectedHead,
    [Parameter(Mandatory)] [Int64] $ExpectedRunId,
    [string] $ExpectedMergeCommit,
    [string] $AncestorOf,
    [switch] $RequireFinalCodeowners,
    [switch] $ValidateExisting,
    [switch] $NoWrite,
    [switch] $SkipCodeowners,
    [int] $PrNumber = 8,
    [string] $ExpectedBase = 'master',
    [string] $ExpectedHeadRef = 'feat/p-mem-1-core'
)

$ErrorActionPreference = 'Stop'
$expectedChecks = @(
    'gate (windows-latest, x64)',
    'gate (windows-11-arm, arm64)',
    'gate (macos-14, arm64)',
    'gate (macos-15-intel, x64)',
    'gate (ubuntu-22.04, x64)',
    'gate (ubuntu-24.04-arm, arm64)',
    'gate-cards'
)

function Invoke-GhJson([string[]] $Arguments) {
    $output = & gh @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "gh failed ($LASTEXITCODE): $($output -join [Environment]::NewLine)"
    }
    return ($output -join [Environment]::NewLine) | ConvertFrom-Json
}

function Assert([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

if ($ValidateExisting) {
    $receipt = Get-Content -Raw -LiteralPath $ReceiptPath | ConvertFrom-Json
} else {
    $pr = Invoke-GhJson @('pr', 'view', [string]$PrNumber, '--json', 'url,state,baseRefName,headRefName,headRefOid,mergeStateStatus,reviewDecision,statusCheckRollup,mergeCommit,mergedBy')
    if ($SkipCodeowners) {
        $ownerResponse = [pscustomobject]@{ sha = $null }
        $ownerText = ''
    } else {
        $ownerResponse = Invoke-GhJson @('api', "repos/FerroxLabs/wayland-nano/contents/CODEOWNERS?ref=$ExpectedHead")
        $ownerText = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(($ownerResponse.content -replace '\s', '')))
    }
    $rules = @($ownerText -split "`r?`n" | Where-Object { $_ -match '^\s*[^#\s]' })
    $gatesRule = @($rules | Where-Object { ($_ -split '\s+')[0] -in @('/gates/**', 'gates/**') }).Count -gt 0
    $agentsRule = @($rules | Where-Object { ($_ -split '\s+')[0] -in @('/agents/**', 'agents/**') }).Count -gt 0
    $ownersRule = @($rules | Where-Object { ($_ -split '\s+')[0] -in @('/CODEOWNERS', 'CODEOWNERS') }).Count -gt 0
    $requiredOwners = @('FerroxLabs', 'TradeCanyon')
    $allRulesHaveOwners = @($rules | Where-Object {
        $parts = @($_ -split '\s+' | Where-Object { $_ })
        $logins = @($parts | Select-Object -Skip 1 | ForEach-Object { $_.TrimStart('@') })
        -not (($requiredOwners | Where-Object { $_ -notin $logins }).Count -eq 0)
    }).Count -eq 0
    $receipt = [ordered]@{
    schema_version = 'phase1-pr8-v1'
    captured_at_utc = [DateTime]::UtcNow.ToString('o')
    pr_number = $PrNumber
    url = $pr.url
    state = $pr.state
    base_ref = $pr.baseRefName
    head_ref = $pr.headRefName
    head_sha = $pr.headRefOid
    merge_state = $pr.mergeStateStatus
    review_decision = if ([string]::IsNullOrWhiteSpace($pr.reviewDecision)) { 'PENDING' } else { $pr.reviewDecision }
    merge_commit_sha = if ($null -eq $pr.mergeCommit) { $null } else { $pr.mergeCommit.oid }
    merged_by = if ($null -eq $pr.mergedBy) { $null } else { $pr.mergedBy.login }
    expected_run_id = $ExpectedRunId
    checks = @($pr.statusCheckRollup | ForEach-Object {
        [ordered]@{
            name = $_.name
            conclusion = $_.conclusion
            status = $_.status
            details_url = $_.detailsUrl
        }
    })
    codeowners = [ordered]@{
        blob_sha = $ownerResponse.sha
        rule_count = $rules.Count
        self = $ownersRule
        gates = $gatesRule
        agents = $agentsRule
        required_logins = $requiredOwners
        all_rules_have_required_logins = $allRulesHaveOwners
    }
    }
}

Assert ($receipt.state -ceq $ExpectedState) "PR state was '$($receipt.state)', expected '$ExpectedState'."
Assert ($receipt.pr_number -eq $PrNumber) "Unexpected PR number."
Assert ($receipt.base_ref -ceq $ExpectedBase) "PR base must be $ExpectedBase."
Assert ($receipt.head_ref -ceq $ExpectedHeadRef) "Unexpected PR head ref."
Assert ($receipt.head_sha -ceq $ExpectedHead) "PR head SHA does not match the immutable expected head."
if ($ExpectedState -ceq 'OPEN') {
    $clean = $receipt.merge_state -ceq 'CLEAN'
    $protectedAwaitingReview =
        $receipt.merge_state -ceq 'BLOCKED' -and $receipt.review_decision -ceq 'REVIEW_REQUIRED'
    Assert ($clean -or $protectedAwaitingReview) "OPEN PR must be CLEAN or protected/BLOCKED only on required review."
} elseif ($ExpectedState -ceq 'MERGED') {
    Assert (-not [string]::IsNullOrWhiteSpace($receipt.merge_commit_sha)) 'MERGED PR must report a merge commit.'
    Assert (-not [string]::IsNullOrWhiteSpace($ExpectedMergeCommit)) 'ExpectedMergeCommit is required for MERGED verification.'
    Assert ($receipt.merge_commit_sha -ceq $ExpectedMergeCommit) 'Observed merge commit differs from expected merge commit.'
    Assert (-not [string]::IsNullOrWhiteSpace($AncestorOf)) 'AncestorOf is required for MERGED verification.'
    & git merge-base --is-ancestor $ExpectedHead $AncestorOf
    Assert ($LASTEXITCODE -eq 0) "Expected PR head is not an ancestor of '$AncestorOf'."
} else {
    throw "Unsupported ExpectedState '$ExpectedState'."
}
Assert ($receipt.checks.Count -eq $expectedChecks.Count) "Expected exactly seven checks."
Assert (@($receipt.checks.name | Sort-Object -Unique).Count -eq $expectedChecks.Count) "Checks contain duplicates."
Assert (-not (Compare-Object ($expectedChecks | Sort-Object) ($receipt.checks.name | Sort-Object))) "Check-name set differs from the required seven legs."
foreach ($check in $receipt.checks) {
    Assert ($check.status -ceq 'COMPLETED') "Check '$($check.name)' is not completed."
    Assert ($check.conclusion -ceq 'SUCCESS') "Check '$($check.name)' is not successful."
    Assert ($check.details_url -match "/actions/runs/$ExpectedRunId/") "Check '$($check.name)' is not bound to run $ExpectedRunId."
}
if (-not $SkipCodeowners) {
    Assert $receipt.codeowners.gates 'CODEOWNERS does not cover gates/**.'
    Assert $receipt.codeowners.agents 'CODEOWNERS does not cover agents/**.'
}
if ($RequireFinalCodeowners) {
    Assert (-not $SkipCodeowners) 'Final CODEOWNERS validation cannot be skipped.'
    Assert ($receipt.codeowners.blob_sha -match '^[0-9a-f]{40}$') 'CODEOWNERS blob SHA is missing or invalid.'
    Assert $receipt.codeowners.self 'CODEOWNERS does not protect itself.'
    Assert ($receipt.codeowners.rule_count -eq 3) 'CODEOWNERS must contain exactly three operative rules.'
    Assert $receipt.codeowners.all_rules_have_required_logins 'Every CODEOWNERS rule must name FerroxLabs and TradeCanyon.'
}

if (-not $ValidateExisting -and -not $NoWrite) {
    $parent = Split-Path -Parent $ReceiptPath
    if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
    $json = $receipt | ConvertTo-Json -Depth 8
    [IO.File]::WriteAllText((Join-Path (Get-Location) $ReceiptPath), $json, [Text.UTF8Encoding]::new($false))
}
Write-Output "Verified PR #$PrNumber at $ExpectedHead with seven successful checks; review decision is $($receipt.review_decision)."
