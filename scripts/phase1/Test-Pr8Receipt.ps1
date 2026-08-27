[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $ReceiptPath,
    [Parameter(Mandatory)] [string] $ExpectedState,
    [Parameter(Mandatory)] [string] $ExpectedHead,
    [Parameter(Mandatory)] [Int64] $ExpectedRunId
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

$pr = Invoke-GhJson @('pr', 'view', '8', '--json', 'url,state,baseRefName,headRefName,headRefOid,mergeStateStatus,reviewDecision,statusCheckRollup')
$ownerResponse = Invoke-GhJson @('api', "repos/FerroxLabs/wayland-nano/contents/CODEOWNERS?ref=$ExpectedHead")
$ownerText = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(($ownerResponse.content -replace '\s', '')))
$rules = @($ownerText -split "`r?`n" | Where-Object { $_ -match '^\s*[^#\s]' })
$gatesRule = @($rules | Where-Object { ($_ -split '\s+')[0] -in @('/gates/**', 'gates/**') }).Count -gt 0
$agentsRule = @($rules | Where-Object { ($_ -split '\s+')[0] -in @('/agents/**', 'agents/**') }).Count -gt 0

$receipt = [ordered]@{
    schema_version = 'phase1-pr8-v1'
    captured_at_utc = [DateTime]::UtcNow.ToString('o')
    pr_number = 8
    url = $pr.url
    state = $pr.state
    base_ref = $pr.baseRefName
    head_ref = $pr.headRefName
    head_sha = $pr.headRefOid
    merge_state = $pr.mergeStateStatus
    review_decision = if ([string]::IsNullOrWhiteSpace($pr.reviewDecision)) { 'PENDING' } else { $pr.reviewDecision }
    expected_run_id = $ExpectedRunId
    checks = @($pr.statusCheckRollup | ForEach-Object {
        [ordered]@{
            name = $_.name
            conclusion = $_.conclusion
            status = $_.status
            details_url = $_.detailsUrl
        }
    })
    codeowners = [ordered]@{ gates = $gatesRule; agents = $agentsRule }
}

Assert ($receipt.state -ceq $ExpectedState) "PR state was '$($receipt.state)', expected '$ExpectedState'."
Assert ($receipt.base_ref -ceq 'master') "PR base must be master."
Assert ($receipt.head_ref -ceq 'feat/p-mem-1-core') "Unexpected PR head ref."
Assert ($receipt.head_sha -ceq $ExpectedHead) "PR head SHA does not match the immutable expected head."
Assert ($receipt.merge_state -ceq 'CLEAN') "PR merge state must be CLEAN."
Assert ($receipt.checks.Count -eq $expectedChecks.Count) "Expected exactly seven checks."
Assert (@($receipt.checks.name | Sort-Object -Unique).Count -eq $expectedChecks.Count) "Checks contain duplicates."
Assert (-not (Compare-Object ($expectedChecks | Sort-Object) ($receipt.checks.name | Sort-Object))) "Check-name set differs from the required seven legs."
foreach ($check in $receipt.checks) {
    Assert ($check.status -ceq 'COMPLETED') "Check '$($check.name)' is not completed."
    Assert ($check.conclusion -ceq 'SUCCESS') "Check '$($check.name)' is not successful."
    Assert ($check.details_url -match "/actions/runs/$ExpectedRunId/") "Check '$($check.name)' is not bound to run $ExpectedRunId."
}
Assert $receipt.codeowners.gates 'CODEOWNERS does not cover gates/**.'
Assert $receipt.codeowners.agents 'CODEOWNERS does not cover agents/**.'

$parent = Split-Path -Parent $ReceiptPath
if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
$json = $receipt | ConvertTo-Json -Depth 8
[IO.File]::WriteAllText((Join-Path (Get-Location) $ReceiptPath), $json, [Text.UTF8Encoding]::new($false))
Write-Output "Verified PR #8 at $ExpectedHead with seven successful checks; human review remains $($receipt.review_decision)."
