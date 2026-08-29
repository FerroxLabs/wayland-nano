[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $ReceiptPath,
    [string] $Repository = 'FerroxLabs/wayland-nano',
    [string] $ExpectedBase = 'master',
    [string] $ExpectedAuthor = 'FerroxLabs',
    [string] $ExpectedReviewer = 'TradeCanyon',
    [switch] $RequireSevenNanoChecks,
    [switch] $RequireMergeCommit,
    [switch] $RequireDefaultOff
)
$ErrorActionPreference = 'Stop'
$requiredChecks = @('gate (windows-latest, x64)','gate (windows-11-arm, arm64)','gate (macos-14, arm64)','gate (macos-15-intel, x64)','gate (ubuntu-22.04, x64)','gate (ubuntu-24.04-arm, arm64)','gate-cards')
$requiredCodeowners = @('/CODEOWNERS @FerroxLabs @TradeCanyon','/gates/** @FerroxLabs @TradeCanyon','/agents/** @FerroxLabs @TradeCanyon')
function Assert([bool]$Condition,[string]$Message){if(-not $Condition){throw $Message}}
function Gh([string[]]$Arguments){$o=& gh @Arguments 2>&1;if($LASTEXITCODE){throw "gh failed ($LASTEXITCODE): $($o -join [Environment]::NewLine)"};($o -join [Environment]::NewLine)|ConvertFrom-Json}
function Git([string[]]$Arguments){$o=& git @Arguments 2>&1;if($LASTEXITCODE){throw "git failed ($LASTEXITCODE): $($o -join [Environment]::NewLine)"};($o -join "`n").Trim()}
function Hash-GitBlob([string]$Object){
    $psi=New-Object Diagnostics.ProcessStartInfo;$psi.FileName='git';$psi.Arguments="cat-file blob $Object";$psi.UseShellExecute=$false;$psi.RedirectStandardOutput=$true;$psi.RedirectStandardError=$true
    $p=[Diagnostics.Process]::Start($psi);$s=[Security.Cryptography.SHA256]::Create()
    try{$h=$s.ComputeHash($p.StandardOutput.BaseStream);$p.WaitForExit();if($p.ExitCode-ne 0){throw "git cat-file failed: $($p.StandardError.ReadToEnd())"};-join($h|ForEach-Object{$_.ToString('x2')})}finally{$s.Dispose();$p.Dispose()}
}

$r=Get-Content -Raw -LiteralPath $ReceiptPath|ConvertFrom-Json
Assert ($r.schema_version -ceq 'phase2-nano-implementation-governance-v1') 'Receipt schema mismatch.'
Assert ($r.repository -ceq $Repository -and $r.base_ref -ceq $ExpectedBase -and $r.author_login -ceq $ExpectedAuthor) 'Receipt repository/base/author mismatch.'
Assert ($r.head_sha -cmatch '^[0-9a-f]{40}$' -and $r.artifact.source_commit_sha -ceq $r.head_sha) 'Immutable source commit binding mismatch.'
Assert ($r.governance_model -ceq 'single-human-distinct-account' -and $r.same_human_controller -and -not $r.independent_human_review -and $r.owner_directed_agent_operated_review -and $r.executor_did_switch_review_merge) 'Compensated-governance disclosure mismatch.'

$pr=Gh @('pr','view',[string]$r.pr_number,'--repo',$Repository,'--json','number,state,baseRefName,headRefName,headRefOid,author,statusCheckRollup,reviews,mergeCommit,mergedBy')
Assert ($pr.state -ceq 'MERGED' -and $pr.baseRefName -ceq $ExpectedBase -and $pr.headRefName -ceq $r.head_ref -and $pr.headRefOid -ceq $r.head_sha -and $pr.author.login -ceq $ExpectedAuthor) 'Live PR exact identity/merged state mismatch.'
$review=@($pr.reviews|Where-Object{$_.id -ceq $r.review.id -and $_.state -ceq 'APPROVED' -and $_.author.login -ceq $ExpectedReviewer -and $_.commit.oid -ceq $r.head_sha})
Assert ($review.Count -eq 1 -and $r.review.reviewer_login -ceq $ExpectedReviewer -and $r.review.commit_oid -ceq $r.head_sha) 'Exact-head compensated approval is missing.'
Assert ($pr.mergedBy.login -ceq $ExpectedReviewer -and $r.merger_login -ceq $ExpectedReviewer -and $pr.mergeCommit.oid -ceq $r.merge_commit_sha) 'Merge attribution/SHA mismatch.'
Git @('fetch','origin',$ExpectedBase,'--quiet')|Out-Null
if($RequireMergeCommit){
    $parents=@((Git @('rev-list','--parents','-n','1',$r.merge_commit_sha))-split '\s+')
    Assert ($parents.Count -eq 3) 'Required merge commit does not have exactly two parents.'
    Assert ($parents[2] -ceq $r.head_sha) 'Reviewed head is not the merge commit second parent.'
}

if($RequireSevenNanoChecks){
    $checks=@($pr.statusCheckRollup|Where-Object{$_.__typename -eq 'CheckRun' -and $_.name -in $requiredChecks})
    Assert ($checks.Count -eq 7 -and @($checks.name|Sort-Object -Unique).Count -eq 7 -and -not(Compare-Object ($checks.name|Sort-Object) ($requiredChecks|Sort-Object))) 'Live required seven-check set mismatch.'
    foreach($c in $checks){Assert($c.status -ceq 'COMPLETED' -and $c.conclusion -ceq 'SUCCESS') "Check '$($c.name)' is not successful.";Assert($c.detailsUrl -match "/actions/runs/$($r.expected_run_id)(?:/|$)") "Check '$($c.name)' run ID mismatch."}
    Assert (@($r.checks).Count -eq 7 -and @($r.checks.name|Sort-Object -Unique).Count -eq 7) 'Receipt seven-check set is incomplete or duplicated.'
    foreach($c in @($r.checks)){
        Assert($c.name -in $requiredChecks -and $c.status -ceq 'COMPLETED' -and $c.conclusion -ceq 'SUCCESS' -and [Int64]$c.run_id -eq [Int64]$r.expected_run_id -and $c.details_url -match "/actions/runs/$($r.expected_run_id)(?:/|$)") "Receipt check '$($c.name)' is not exact and successful."
        $live=@($checks|Where-Object{$_.name -ceq $c.name -and $_.detailsUrl -ceq $c.details_url})
        Assert($live.Count -eq 1) "Receipt check '$($c.name)' does not match the unique live check."
    }
}

$protection=Gh @('api',"repos/$Repository/branches/$ExpectedBase/protection")
$contexts=@($protection.required_status_checks.contexts)
Assert($protection.required_status_checks.strict -and $contexts.Count -eq 7 -and -not(Compare-Object ($contexts|Sort-Object) ($requiredChecks|Sort-Object))) 'Live strict check protection mismatch.'
Assert($protection.required_pull_request_reviews.require_code_owner_reviews -and $protection.required_pull_request_reviews.required_approving_review_count -eq 1 -and $protection.required_pull_request_reviews.dismiss_stale_reviews -and $protection.required_pull_request_reviews.require_last_push_approval) 'Live review protection mismatch.'
Assert($protection.enforce_admins.enabled -and $protection.required_conversation_resolution.enabled -and -not $protection.allow_force_pushes.enabled -and -not $protection.allow_deletions.enabled) 'Live no-bypass protection mismatch.'
Assert($r.protection.strict -and $r.protection.required_status_checks -eq 7 -and $r.protection.required_codeowner_approvals -eq 1 -and $r.protection.dismiss_stale_reviews -and $r.protection.require_last_push_approval -and $r.protection.require_conversation_resolution -and $r.protection.enforce_admins -and -not $r.protection.allow_force_pushes -and -not $r.protection.allow_deletions) 'Receipt protection evidence mismatch.'
Assert(@(Gh @('api',"repos/$Repository/rulesets")).Count -eq 0 -and @($r.protection.bypass_actors).Count -eq 0) 'Protection bypass actors/rulesets must be absent.'

& git merge-base --is-ancestor $r.head_sha $r.merge_commit_sha
Assert($LASTEXITCODE -eq 0) 'Reviewed head is not an ancestor of merge commit.'
& git merge-base --is-ancestor $r.merge_commit_sha "origin/$ExpectedBase"
Assert($LASTEXITCODE -eq 0) 'Merge commit is not an ancestor of fresh origin/base.'
$rules=@((Git @('show',"$($r.merge_commit_sha)`:CODEOWNERS"))-split "`r?`n"|ForEach-Object{$_.Trim()}|Where-Object{$_ -and -not $_.StartsWith('#')})
Assert($rules.Count -eq 3 -and -not(Compare-Object $rules $requiredCodeowners)) 'Merged exact CODEOWNERS rules mismatch.'
Assert((Git @('rev-parse',"$($r.head_sha)`:CODEOWNERS")) -ceq $r.codeowners.blob_sha) 'Receipt CODEOWNERS blob mismatch.'

Assert((Hash-GitBlob "$($r.head_sha)`:Cargo.lock") -ceq $r.artifact.cargo_lock_sha256 -and (Git @('rev-parse',"$($r.head_sha)`:Cargo.lock")) -ceq $r.artifact.cargo_lock_blob_sha) 'Reviewed Cargo.lock digest/blob mismatch.'
Assert((Git @('rev-parse',"$($r.merge_commit_sha)`:Cargo.lock")) -ceq $r.artifact.cargo_lock_blob_sha) 'Merged Cargo.lock differs from reviewed artifact.'
if($RequireDefaultOff){
    Assert($r.default_off.required -and $r.default_off.missing_state_refuses) 'Receipt does not assert default-off.'
    Assert((Git @('rev-parse',"$($r.merge_commit_sha)`:crates/nano-activation/src/enablement.rs")) -ceq $r.default_off.implementation_blob_sha) 'Merged enablement implementation differs from reviewed default-off artifact.'
    Assert((Git @('rev-parse',"$($r.merge_commit_sha)`:crates/nano-activation/tests/enablement.rs")) -ceq $r.default_off.test_blob_sha) 'Merged default-off test differs from reviewed artifact.'
    $text=Git @('show',"$($r.merge_commit_sha)`:crates/nano-activation/src/enablement.rs")
    Assert($text -match 'replay\(\)\?\.ok_or\(EnablementError::Missing\)') 'Merged implementation no longer refuses absent enablement.'
}
Write-Output "PASS: protected exact-head compensated Nano governance verified at merge $($r.merge_commit_sha)"
