[CmdletBinding()]
param(
 [string]$ReceiptPath='.planning/phases/01-ownership-contract-and-foundation/evidence/foundation-prerequisites.json',
 [string]$AcceptancePath='.planning/phases/01-ownership-contract-and-foundation/evidence/foundation-acceptance-full.json'
)
$ErrorActionPreference='Stop'
function Hash([string]$p){(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant()}
$evidenceDir=Split-Path $ReceiptPath;$prePath=Join-Path $evidenceDir 'pr-8-premerge.json';$pre=Get-Content -Raw $prePath|ConvertFrom-Json
git fetch origin master --quiet;$master=(git rev-parse origin/master).Trim();$pr=gh pr view 8 --repo FerroxLabs/wayland-nano --json state,mergeCommit|ConvertFrom-Json
if($pr.state-ne'MERGED'-or-not$pr.mergeCommit.oid){throw 'PR #8 is not merged'}
$mergedPath=Join-Path $evidenceDir 'merged-pr-8.json'
& "$PSScriptRoot/Test-Pr8Receipt.ps1" -ReceiptPath $mergedPath -ExpectedState MERGED -ExpectedHead $pre.head_sha -ExpectedRunId ([int64]$pre.expected_run_id) -ExpectedMergeCommit $pr.mergeCommit.oid -AncestorOf $master -RequireFinalCodeowners;if($LASTEXITCODE){exit $LASTEXITCODE}
& "$PSScriptRoot/Test-FoundationCorrectiveLanes.ps1" -ReceiptPath (Join-Path $evidenceDir 'foundation-corrective-lanes.json');if($LASTEXITCODE){exit $LASTEXITCODE}
& "$PSScriptRoot/Test-AuthorityRatification.ps1" -ReceiptPath (Join-Path $evidenceDir 'authority-amendment-ratification.json') -ArtifactPath 'D:/Development/waylandnano/shared/reviews/research-0.2/specs/WORKABLE-AGENT-AUTHORITY-AMENDMENT-v1.0.md' -PremergeReceiptPath $prePath -ExpectedVersion v1.0;if($LASTEXITCODE){exit $LASTEXITCODE}
& "$PSScriptRoot/Test-HumanGovernance.ps1" -ReceiptPath (Join-Path $evidenceDir 'human-checkpoints.json') -PremergeReceiptPath $prePath -BootstrapReceiptPath (Join-Path $evidenceDir 'pr-10-bootstrap.json') -ExpectedBase master -ExpectedAuthor FerroxLabs -ExpectedReviewer TradeCanyon;if($LASTEXITCODE){exit $LASTEXITCODE}
& "$PSScriptRoot/Test-PmemAcceptanceReceipt.ps1" -ReceiptPath $AcceptancePath -ExpectedSha $master -RequireWorkspace;if($LASTEXITCODE){exit $LASTEXITCODE}
$pairs=@(
 @('NANO-PROGRAM-PLAN','D:/Development/waylandnano/shared/reviews/research-0.2/NANO-PROGRAM-PLAN.md','.planning/sources/NANO-PROGRAM-PLAN.md'),
 @('MEMORY-CONTRACT','D:/Development/waylandnano/shared/reviews/research-0.2/specs/MEMORY-CONTRACT.md','.planning/sources/MEMORY-CONTRACT.md'),
 @('PROFILES-CONTRACT','D:/Development/waylandnano/shared/reviews/research-0.2/specs/PROFILES-CONTRACT.md','.planning/sources/PROFILES-CONTRACT.md'),
 @('NANO-MODULE-CONTRACT','D:/Development/waylandnano/shared/reviews/research-0.2/specs/NANO-MODULE-CONTRACT.md','.planning/sources/NANO-MODULE-CONTRACT.md'))
$sourcePairs=@();foreach($pair in $pairs){$a=Hash $pair[1];$b=Hash $pair[2];if($a-ne$b){throw "Source snapshot mismatch: $($pair[0])"};$sourcePairs+=[ordered]@{name=$pair[0];source_path=$pair[1];snapshot_path=$pair[2];source_sha256=$a;snapshot_sha256=$b;byte_equal=$true}}
$authPath=Join-Path $evidenceDir 'authority-amendment-ratification.json';$auth=Get-Content -Raw $authPath|ConvertFrom-Json
$receipt=[ordered]@{schema_version='foundation-prerequisites-v1';captured_at_utc=[DateTime]::UtcNow.ToString('o');origin_master_sha=$master;pr8_head_sha=$pre.head_sha;pr8_run_id=[int64]$pre.expected_run_id;merge_commit=$pr.mergeCommit.oid;head_is_origin_master_ancestor=$true;signed_amendment=[ordered]@{path=$auth.artifact_path;sha256=$auth.final_signed_artifact_sha256;byte_length=[int64]$auth.artifact_byte_length;receipt_sha256=Hash $authPath};source_pairs=$sourcePairs;receipts=[ordered]@{merged_pr8_sha256=Hash $mergedPath;bootstrap_pr10_sha256=Hash (Join-Path $evidenceDir 'pr-10-bootstrap.json');bootstrap_pr11_sha256=Hash (Join-Path $evidenceDir 'pr-11-bootstrap.json');corrective_lanes_sha256=Hash (Join-Path $evidenceDir 'foundation-corrective-lanes.json');cross_ai_sha256=Hash (Join-Path $evidenceDir 'cross-ai-audit-receipt.json');human_governance_sha256=Hash (Join-Path $evidenceDir 'human-checkpoints.json');foundation_acceptance_sha256=Hash $AcceptancePath};governance=[ordered]@{same_human_controller=$true;independent_human_review=$false;owner_directed_agent_operated_review=$true;executor_did_switch_review_merge=$true};passed=$true}
$receipt|ConvertTo-Json -Depth 9|Set-Content -LiteralPath $ReceiptPath -Encoding utf8
Write-Host "PASS: Phase 1 prerequisites bind signed amendment, merged head, sources, governance, CI, and eight-command acceptance at $master"
