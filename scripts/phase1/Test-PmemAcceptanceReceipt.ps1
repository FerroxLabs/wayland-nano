[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $ReceiptPath,
    [Parameter(Mandatory)] [string] $ExpectedSha,
    [switch] $RequireWorkspace
)

$ErrorActionPreference = 'Stop'
function Assert([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw $Message }
}

$base = @(
    [ordered]@{ id = 'retrieval_recall'; argv = @('cargo','test','-p','nano-memory','--test','retrieval_recall','--','--nocapture') },
    [ordered]@{ id = 'durability'; argv = @('cargo','test','-p','nano-memory','--test','durability','--','--nocapture') },
    [ordered]@{ id = 'write_mediation'; argv = @('cargo','test','-p','nano-memory','--test','write_mediation','--','--nocapture') },
    [ordered]@{ id = 'nano_memory'; argv = @('cargo','test','-p','nano-memory') },
    [ordered]@{ id = 'nano_session'; argv = @('cargo','test','-p','nano-session') },
    [ordered]@{ id = 'fmt'; argv = @('cargo','fmt','--all','--','--check') },
    [ordered]@{ id = 'clippy'; argv = @('cargo','clippy','-p','nano-memory','-p','nano-session','--all-targets','--','-D','warnings') }
)
if ($RequireWorkspace) { $base += [ordered]@{ id = 'workspace_test'; argv = @('cargo','test','--workspace') } }

$receipt = Get-Content -Raw -LiteralPath $ReceiptPath | ConvertFrom-Json
Assert ($receipt.schema_version -ceq 'pmem-acceptance-v1') 'Unexpected receipt schema.'
Assert ($receipt.tested_sha -ceq $ExpectedSha) 'Receipt tested_sha does not match expected SHA.'
Assert ([bool]$receipt.include_workspace -eq [bool]$RequireWorkspace) 'Workspace mode does not match verifier mode.'
Assert ($receipt.commands.Count -eq $base.Count) "Expected exactly $($base.Count) commands."
Assert (@($receipt.commands.id | Sort-Object -Unique).Count -eq $base.Count) 'Command IDs contain duplicates.'
for ($index = 0; $index -lt $base.Count; $index++) {
    $actual = $receipt.commands[$index]
    $expected = $base[$index]
    Assert ($actual.id -ceq $expected.id) "Command $index has unexpected ID."
    Assert ($actual.pre_command_sha -ceq $ExpectedSha) "Command '$($actual.id)' ran at the wrong SHA."
    Assert ([int]$actual.exit_code -eq 0) "Command '$($actual.id)' failed."
    $difference = Compare-Object @($expected.argv) @($actual.argv) -SyncWindow 0
    Assert (-not $difference) "Command '$($actual.id)' argv differs from the locked manifest."
}
Assert ([double]$receipt.metrics.recall_at_10 -ge 0.90) 'recall@10 is below 0.90 or missing.'
Assert ([int]$receipt.metrics.cross_project_rows -eq 0) 'Cross-project leakage is non-zero or missing.'
Assert ([int]$receipt.metrics.cross_agent_rows -eq 0) 'Cross-agent leakage is non-zero or missing.'
Assert ([bool]$receipt.metrics.kill_rebuild_query_equivalent) 'Kill/rebuild query equivalence was not proven.'
Assert ([bool]$receipt.metrics.kill_rebuild_includes_agent_id) 'Kill/rebuild agent_id preservation was not proven.'
Assert ([bool]$receipt.metrics.mediation_receipt_visible) 'Write-mediation receipt was not proven.'
Write-Output "Verified P-MEM acceptance receipt for $ExpectedSha with $($base.Count) exact commands."
