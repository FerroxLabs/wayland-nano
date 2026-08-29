[CmdletBinding()]
param(
  [string]$ReceiptPath = '.planning/phases/01-ownership-contract-and-foundation/evidence/pr-10-bootstrap.json',
  [string]$Pr11ReceiptPath = '.planning/phases/01-ownership-contract-and-foundation/evidence/pr-11-bootstrap.json'
)
$ErrorActionPreference = 'Stop'
$repo = 'FerroxLabs/wayland-nano'
$expectedChecks = @('gate (windows-latest, x64)','gate (windows-11-arm, arm64)','gate (macos-14, arm64)','gate (macos-15-intel, x64)','gate (ubuntu-22.04, x64)','gate (ubuntu-24.04-arm, arm64)','gate-cards')
function Get-PrEvidence([int]$Number) {
  $pr = gh pr view $Number --repo $repo --json number,state,headRefOid,baseRefName,author,reviews,mergeCommit,mergedAt,mergedBy,statusCheckRollup,url | ConvertFrom-Json
  if ($pr.state -ne 'MERGED' -or $pr.baseRefName -ne 'master' -or $pr.author.login -ne 'FerroxLabs') { throw "PR #$Number is not the expected merged bootstrap PR" }
  if ($pr.mergedBy.login -ne 'TradeCanyon' -or -not $pr.mergeCommit.oid) { throw "PR #$Number merge attribution is invalid" }
  $review = @($pr.reviews | Where-Object { $_.author.login -eq 'TradeCanyon' -and $_.state -eq 'APPROVED' -and $_.commit.oid -eq $pr.headRefOid })[-1]
  if (-not $review) { throw "PR #$Number lacks exact-head TradeCanyon approval" }
  $checks = @($pr.statusCheckRollup | Where-Object { $_.__typename -eq 'CheckRun' })
  foreach ($name in $expectedChecks) {
    $matches = @($checks | Where-Object { $_.name -eq $name -and $_.status -eq 'COMPLETED' -and $_.conclusion -eq 'SUCCESS' })
    if ($matches.Count -ne 1) { throw "PR #$Number missing unique successful check: $name" }
  }
  if (@($checks | Where-Object { $_.name -in $expectedChecks }).Count -ne 7) { throw "PR #$Number does not have exactly seven required checks" }
  $runIds = @($checks.detailsUrl | ForEach-Object { if ($_ -match '/actions/runs/(\d+)/') { [int64]$Matches[1] } } | Sort-Object -Unique)
  if ($runIds.Count -ne 1) { throw "PR #$Number checks do not bind one run" }
  [ordered]@{
    schema_version = "phase1-pr$Number-bootstrap-v1"; pr_number = $Number; url = $pr.url
    state = $pr.state; base_ref = $pr.baseRefName; author_login = $pr.author.login
    head_sha = $pr.headRefOid; expected_run_id = $runIds[0]
    review_id = $review.id; review_commit_oid = $review.commit.oid
    reviewer_login = $review.author.login; merger_login = $pr.mergedBy.login
    merge_commit_sha = $pr.mergeCommit.oid; merged_at = $pr.mergedAt
    governance_model = 'single-human-distinct-account'; same_human_controller = $true
    independent_human_review = $false; owner_directed_agent_operated_review = $true
    executor_did_switch_review_merge = $true
    checks = @($checks | Where-Object name -in $expectedChecks | Sort-Object name | ForEach-Object { [ordered]@{name=$_.name;status=$_.status;conclusion=$_.conclusion;details_url=$_.detailsUrl} })
  }
}
$pr10 = Get-PrEvidence 10
$pr11 = Get-PrEvidence 11
$codeowners = git show "$($pr10.merge_commit_sha):CODEOWNERS"
$expected = @('/CODEOWNERS @FerroxLabs @TradeCanyon','/gates/** @FerroxLabs @TradeCanyon','/agents/** @FerroxLabs @TradeCanyon')
$rules = @($codeowners -split "`n" | ForEach-Object {$_.Trim()} | Where-Object { $_ -and -not $_.StartsWith('#') })
if ($rules.Count -ne 3 -or (Compare-Object $rules $expected)) { throw 'PR #10 merge does not contain the exact three CODEOWNERS rules' }
$pr10.codeowners_blob_sha = (git rev-parse "$($pr10.merge_commit_sha):CODEOWNERS").Trim()
$pr10.codeowners_rules = $rules
New-Item -ItemType Directory -Force -Path (Split-Path $ReceiptPath) | Out-Null
$pr10 | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ReceiptPath -Encoding utf8
$pr11 | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $Pr11ReceiptPath -Encoding utf8
Write-Host "PASS: PR #10/#11 bootstrap receipts regenerated from live GitHub evidence"
