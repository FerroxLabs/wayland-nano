[CmdletBinding()]
param(
  [Parameter(Mandatory)][string]$ReceiptPath,
  [Parameter(Mandatory)][string]$ArtifactPath,
  [Parameter(Mandatory)][string]$PremergeReceiptPath,
  [string]$ExpectedVersion='v1.0'
)
$ErrorActionPreference='Stop'
function Hash([string]$p){(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant()}
function Reject-Legacy($value){
  $json=$value|ConvertTo-Json -Depth 20
  foreach($term in @('human_interactive_review','hardware_mfa','passkey','credential_inaccessibility','executor_nonparticipation','executor_did_not_switch','independent_human_review": true')){if($json -match [regex]::Escape($term)){throw "Superseded governance claim present: $term"}}
}
$r=Get-Content -Raw -LiteralPath $ReceiptPath|ConvertFrom-Json
$pre=Get-Content -Raw -LiteralPath $PremergeReceiptPath|ConvertFrom-Json
$artifact=(Resolve-Path -LiteralPath $ArtifactPath).Path
$bytes=[IO.File]::ReadAllBytes($artifact);$sha=Hash $artifact
if($r.schema_version -ne 'wayland.nano.amendment-ratification/v1' -or $r.amendment_version -ne $ExpectedVersion){throw 'Ratification schema/version mismatch'}
if($r.artifact_path -replace '\\','/' -ne ($ArtifactPath -replace '\\','/')){throw 'Ratification artifact path mismatch'}
if($r.final_signed_artifact_sha256 -ne $sha -or $r.artifact_sha256 -ne $sha -or [int64]$r.artifact_byte_length -ne $bytes.Length){throw 'Signed amendment SHA or length mismatch'}
foreach($field in @('owner_name','owner_signature','signature_date','owner_acceptance_signature')){if([string]::IsNullOrWhiteSpace($r.$field) -or $r.$field -match 'PENDING'){throw "Missing owner ratification field: $field"}}
if($r.pr_head_sha -ne $pre.head_sha){throw 'Ratification is not bound to final PR #8 head'}
if($r.governance_model -ne 'single-human-distinct-account' -or -not $r.same_human_controller -or $r.independent_human_review -or -not $r.owner_directed_agent_operated_review -or -not $r.executor_did_switch_review_merge -or -not $r.sybil_collusion_residual_accepted){throw 'Governance disclosure is incomplete or dishonest'}
$crossPath=Join-Path (Split-Path $ReceiptPath) 'cross-ai-audit-receipt.json';$cross=Get-Content -Raw $crossPath|ConvertFrom-Json
$promptPath=Join-Path (Split-Path $ReceiptPath) 'cross-ai-audit-prompt.txt'
if($cross.pr_head_sha -ne $pre.head_sha -or $cross.audit_prompt_sha256 -ne (Hash $promptPath) -or $r.audit_prompt_sha256 -ne $cross.audit_prompt_sha256 -or $r.amendment_candidate_sha256 -ne $cross.amendment_candidate_sha256){throw 'Cross-AI binding mismatch'}
$lineages=@($cross.reviews|Where-Object {$_.status -eq 'completed' -and $_.verdict -eq 'PASS' -and $_.provider_lineage -ne $r.caller_lineage -and $_.provider_lineage -ne $r.implementation_lineage}|ForEach-Object {$_.provider_lineage}|Sort-Object -Unique)
if(-not $cross.quorum_met -or $lineages.Count -lt 2){throw 'Cross-AI quorum is not met'}
foreach($review in $cross.reviews){$raw=Join-Path (Split-Path $ReceiptPath) ($review.raw_output_path -replace '^\.planning/phases/01-ownership-contract-and-foundation/evidence/','');if(-not(Test-Path $raw)-or (Hash $raw)-ne $review.raw_output_sha256){throw "Cross-AI raw output mismatch: $($review.id)"}}
foreach($finding in @($cross.critical_high_dispositions)){if($finding.status -notin @('fixed_reaudited','owner_accepted')){throw "Unresolved audit finding: $($finding.id)"}}
if(@($r.cross_ai_reviews).Count -lt 2 -or @($r.completed_distinct_provider_lineages).Count -lt 2 -or $null -eq $r.disqualified_reviews -or $null -eq $r.critical_high_dispositions){throw 'Detached ratification receipt omits required cross-AI evidence fields'}
Reject-Legacy $r
Write-Host "PASS: final signed amendment $sha is ratified and bound to PR #8 $($pre.head_sha)"
