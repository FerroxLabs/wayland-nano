[CmdletBinding()]
param(
 [Parameter(Mandatory)][string]$ReceiptPath,
 [Parameter(Mandatory)][string]$PremergeReceiptPath,
 [Parameter(Mandatory)][string]$BootstrapReceiptPath,
 [string]$ExpectedBase='master',[string]$ExpectedAuthor='FerroxLabs',[string]$ExpectedReviewer='TradeCanyon'
)
$ErrorActionPreference='Stop';$repo='FerroxLabs/wayland-nano'
function Hash([string]$p){(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant()}
$evidenceDir=Split-Path $ReceiptPath;$r=Get-Content -Raw $ReceiptPath|ConvertFrom-Json;$pre=Get-Content -Raw $PremergeReceiptPath|ConvertFrom-Json;$boot=Get-Content -Raw $BootstrapReceiptPath|ConvertFrom-Json
& "$PSScriptRoot/New-Pr10BootstrapReceipt.ps1" -ReceiptPath $BootstrapReceiptPath -Pr11ReceiptPath (Join-Path $evidenceDir 'pr-11-bootstrap.json');if($LASTEXITCODE){exit $LASTEXITCODE}
$boot=Get-Content -Raw $BootstrapReceiptPath|ConvertFrom-Json;$pr11Path=Join-Path $evidenceDir 'pr-11-bootstrap.json';$pr11=Get-Content -Raw $pr11Path|ConvertFrom-Json
& "$PSScriptRoot/Test-AuthorityRatification.ps1" -ReceiptPath (Join-Path $evidenceDir 'authority-amendment-ratification.json') -ArtifactPath 'D:/Development/waylandnano/shared/reviews/research-0.2/specs/WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md' -PremergeReceiptPath $PremergeReceiptPath -ExpectedVersion v1.0;if($LASTEXITCODE){exit $LASTEXITCODE}
if($r.schema_version -ne 'human-checkpoints-v2' -or $r.author_login -ne $ExpectedAuthor -or $r.reviewer_login -ne $ExpectedReviewer -or $r.merger_login -ne $ExpectedReviewer -or $r.base_branch -ne $ExpectedBase){throw 'Governance receipt identity mismatch'}
if($r.governance_model -ne 'single-human-distinct-account' -or -not $r.same_human_controller -or $r.independent_human_review -or -not $r.owner_directed_agent_operated_review -or -not $r.executor_did_switch_review_merge){throw 'Governance receipt disclosures invalid'}
if($r.pr10.head_sha -ne $boot.head_sha -or $r.pr10.review_id -ne $boot.review_id -or $r.pr10.merge_commit_sha -ne $boot.merge_commit_sha){throw 'PR #10 receipt binding mismatch'}
if($r.pr11.head_sha -ne $pr11.head_sha -or $r.pr11.review_id -ne $pr11.review_id -or $r.pr11.merge_commit_sha -ne $pr11.merge_commit_sha){throw 'PR #11 receipt binding mismatch'}
if(-not $boot.collaborator.invitation_accepted -or $boot.collaborator.invitation_id -ne 330736811 -or $boot.collaborator.permission -ne 'write' -or -not $boot.collaborator.push -or -not $pr11.collaborator.invitation_accepted -or $pr11.fixture_path -ne 'gates/fixtures/memory-retrieval-recall-v1/fixture.json' -or $pr11.fixture_blob_sha -notmatch '^[0-9a-f]{40}$' -or $pr11.validator.path -ne 'gates/validate-memory-recall-fixture.cjs' -or $pr11.validator.blob_sha -notmatch '^[0-9a-f]{40}$'){throw 'Bootstrap collaborator or fixture evidence is incomplete'}
if($r.pr10_bootstrap_receipt_path -ne '.planning/phases/01-ownership-contract-and-foundation/evidence/pr-10-bootstrap.json' -or $r.pr11_bootstrap_receipt_path -ne '.planning/phases/01-ownership-contract-and-foundation/evidence/pr-11-bootstrap.json' -or $r.pr10_bootstrap_receipt_sha256-ne(Hash $BootstrapReceiptPath)-or$r.pr11_bootstrap_receipt_sha256-ne(Hash $pr11Path)){throw 'Bootstrap receipt path/hash binding mismatch'}
$pr=gh pr view 8 --repo $repo --json state,headRefOid,baseRefName,author,reviews,mergeCommit,mergedBy|ConvertFrom-Json
if($pr.state-ne'MERGED'-or $pr.headRefOid-ne$pre.head_sha-or $r.expected_head_sha-ne$pre.head_sha-or$r.observed_head_sha-ne$pr.headRefOid-or $pr.baseRefName-ne$ExpectedBase-or $pr.author.login-ne$ExpectedAuthor-or $pr.mergedBy.login-ne$ExpectedReviewer-or $pr.mergeCommit.oid-ne$r.pr8.merge_commit_sha){throw 'Live PR #8 merge state/identity mismatch'}
$review=@($pr.reviews|Where-Object {$_.id-eq$r.pr8.review_id-and$_.state-eq'APPROVED'-and$_.author.login-eq$ExpectedReviewer-and$_.commit.oid-eq$pre.head_sha})
if($review.Count-ne 1){throw 'Exact-head PR #8 TradeCanyon approval missing'}
git fetch origin master --quiet;git merge-base --is-ancestor $pre.head_sha $pr.mergeCommit.oid;if($LASTEXITCODE){throw 'Reviewed head is not ancestor of merge commit'}
$protection=gh api "repos/$repo/branches/master/protection"|ConvertFrom-Json
$contexts=@($protection.required_status_checks.contexts);if(-not$protection.required_status_checks.strict-or$contexts.Count-ne 7-or-not$protection.required_pull_request_reviews.require_code_owner_reviews-or$protection.required_pull_request_reviews.required_approving_review_count-ne 1-or-not$protection.required_pull_request_reviews.dismiss_stale_reviews-or-not$protection.required_pull_request_reviews.require_last_push_approval-or-not$protection.enforce_admins.enabled-or-not$protection.required_conversation_resolution.enabled-or$protection.allow_force_pushes.enabled-or$protection.allow_deletions.enabled){throw 'Classic master branch protection mismatch'}
$rulesets=@(gh api "repos/$repo/rulesets"|ConvertFrom-Json);if($rulesets.Count-ne 0 -or @($r.protection.bypass_actors).Count-ne 0){throw 'Protection bypass actors must be empty'}
$co=git show "origin/master:CODEOWNERS";$rules=@($co-split"`n"|%{$_.Trim()}|?{$_-and-not$_.StartsWith('#')});$expected=@('/CODEOWNERS @FerroxLabs @TradeCanyon','/gates/** @FerroxLabs @TradeCanyon','/agents/** @FerroxLabs @TradeCanyon');if($rules.Count-ne 3-or(Compare-Object $rules $expected)){throw 'Exact CODEOWNERS rules mismatch'}
$authPath=Join-Path $evidenceDir 'authority-amendment-ratification.json';$auth=Get-Content -Raw $authPath|ConvertFrom-Json;$crossPath=Join-Path $evidenceDir 'cross-ai-audit-receipt.json';$cross=Get-Content -Raw $crossPath|ConvertFrom-Json;$acceptancePath=Join-Path $evidenceDir 'foundation-acceptance-full.json'
$masterCodeownersBlob=(git rev-parse 'origin/master:CODEOWNERS').Trim()
if($r.premerge_receipt_sha256 -ne (Hash $PremergeReceiptPath) -or $r.codeowners_blob_sha -ne $masterCodeownersBlob -or $r.signed_artifact_sha256 -ne $auth.final_signed_artifact_sha256 -or $r.authority_receipt_sha256 -ne (Hash $authPath) -or $r.cross_ai_receipt_sha256 -ne (Hash $crossPath) -or $r.audit_prompt_sha256 -ne $cross.audit_prompt_sha256 -or $r.foundation_acceptance_sha256 -ne (Hash $acceptancePath)){throw 'Governance file hash/blob binding mismatch'}
$captured=[datetimeoffset]::Parse([string]$r.captured_at_utc)
$authTime=[datetimeoffset]::Parse([string]$auth.receipt_time_utc)
$crossTime=[datetimeoffset]::Parse([string]$cross.completed_at_utc)
$acceptance=Get-Content -Raw $acceptancePath|ConvertFrom-Json
$acceptanceTime=[datetimeoffset]::Parse([string]$acceptance.finished_at_utc)
if($captured-lt$authTime-or$captured-lt$crossTime-or$captured-lt$acceptanceTime){throw 'Governance receipt predates bound evidence'}
$legacy=$r|ConvertTo-Json -Depth 20;foreach($term in @('human_interactive_review','hardware_mfa','passkey','credential_inaccessibility','executor_nonparticipation')){if($legacy-match[regex]::Escape($term)){throw "Stale governance claim: $term"}}
Write-Host 'PASS: owner-directed agent-operated governance and protected exact-head merge verified live'
