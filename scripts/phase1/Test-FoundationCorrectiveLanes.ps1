[CmdletBinding()]
param([string]$ReceiptPath = '.planning/phases/01-ownership-contract-and-foundation/evidence/foundation-corrective-lanes.json')
$ErrorActionPreference='Stop'
& "$PSScriptRoot/New-Pr10BootstrapReceipt.ps1"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$evidenceDir = Split-Path $ReceiptPath
$pr10Path = Join-Path $evidenceDir 'pr-10-bootstrap.json'; $pr11Path = Join-Path $evidenceDir 'pr-11-bootstrap.json'
$pr10 = Get-Content -Raw $pr10Path | ConvertFrom-Json; $pr11 = Get-Content -Raw $pr11Path | ConvertFrom-Json
git fetch origin master --quiet
$master = (git rev-parse origin/master).Trim()
foreach ($sha in @($pr10.merge_commit_sha,$pr11.merge_commit_sha)) { git merge-base --is-ancestor $sha $master; if ($LASTEXITCODE) { throw "$sha is not landed on origin/master" } }
$requiredFiles = @('gates/fixtures/memory-retrieval-recall-v1/fixture.json','crates/nano-memory/tests/corrective_regressions.rs','crates/nano-memory/src/store.rs')
foreach($path in $requiredFiles){ git cat-file -e "${master}:$path"; if($LASTEXITCODE){throw "Missing corrective artifact on master: $path"} }
$fixtureSha=(git rev-parse "${master}:gates/fixtures/memory-retrieval-recall-v1/fixture.json").Trim()
$storeSha=(git rev-parse "${master}:crates/nano-memory/src/store.rs").Trim()
$testsSha=(git rev-parse "${master}:crates/nano-memory/tests/corrective_regressions.rs").Trim()
$prePath=Join-Path $evidenceDir 'pr-8-premerge.json';$pre=Get-Content -Raw $prePath|ConvertFrom-Json
$pr8=gh pr view 8 --repo FerroxLabs/wayland-nano --json state,headRefOid,mergeCommit,statusCheckRollup|ConvertFrom-Json
if($pr8.state-ne'MERGED'-or$pr8.headRefOid-ne$pre.head_sha-or-not$pr8.mergeCommit.oid){throw 'Final P-MEM corrective PR #8 is not merged at the receipted head'}
git merge-base --is-ancestor $pre.head_sha $master;if($LASTEXITCODE){throw 'Final P-MEM head is not on origin/master'}
$expectedChecks=@('gate (windows-latest, x64)','gate (windows-11-arm, arm64)','gate (macos-14, arm64)','gate (macos-15-intel, x64)','gate (ubuntu-22.04, x64)','gate (ubuntu-24.04-arm, arm64)','gate-cards')
$checks=@($pr8.statusCheckRollup|Where-Object {$_.name-in$expectedChecks});if($checks.Count-ne 7-or@($checks|Where-Object {$_.status-ne'COMPLETED'-or$_.conclusion-ne'SUCCESS'}).Count){throw 'Final P-MEM PR lacks the exact seven successful checks'}
$acceptancePath=Join-Path $evidenceDir 'foundation-acceptance-full.json'
$receipt=[ordered]@{schema_version='foundation-corrective-lanes-v1';captured_at_utc=[DateTime]::UtcNow.ToString('o');origin_master_sha=$master;pr10=[ordered]@{head_sha=$pr10.head_sha;run_id=$pr10.expected_run_id;merge_commit_sha=$pr10.merge_commit_sha;receipt_sha256=(Get-FileHash $pr10Path -Algorithm SHA256).Hash.ToLowerInvariant()};pr11=[ordered]@{head_sha=$pr11.head_sha;run_id=$pr11.expected_run_id;merge_commit_sha=$pr11.merge_commit_sha;receipt_sha256=(Get-FileHash $pr11Path -Algorithm SHA256).Hash.ToLowerInvariant()};pr8=[ordered]@{head_sha=$pre.head_sha;run_id=[int64]$pre.expected_run_id;merge_commit_sha=$pr8.mergeCommit.oid;premerge_receipt_sha256=(Get-FileHash $prePath -Algorithm SHA256).Hash.ToLowerInvariant();foundation_acceptance_sha256=(Get-FileHash $acceptancePath -Algorithm SHA256).Hash.ToLowerInvariant();checks=7};artifacts=[ordered]@{fixture_blob_sha=$fixtureSha;store_blob_sha=$storeSha;corrective_tests_blob_sha=$testsSha};owner_directed_agent_operated_review=$true;executor_did_switch_review_merge=$true;all_lanes_landed=$true}
$receipt|ConvertTo-Json -Depth 7|Set-Content -LiteralPath $ReceiptPath -Encoding utf8
Write-Host 'PASS: bootstrap, fixture, and corrective implementation lanes are landed'
