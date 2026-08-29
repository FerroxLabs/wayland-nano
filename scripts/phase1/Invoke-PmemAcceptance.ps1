[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Repository,
    [Parameter(Mandatory)] [string] $ExpectedSha,
    [Parameter(Mandatory)] [string] $OutputPath,
    [switch] $IncludeWorkspace
)

$ErrorActionPreference = 'Stop'
$repositoryPath = (Resolve-Path -LiteralPath $Repository).Path

function Get-HeadSha {
    $sha = (& git -C $repositoryPath rev-parse HEAD 2>&1) -join ''
    if ($LASTEXITCODE -ne 0) { throw "Unable to read repository HEAD: $sha" }
    return $sha.Trim()
}

function Assert-ExpectedHead {
    $sha = Get-HeadSha
    if ($sha -cne $ExpectedSha) { throw "Repository HEAD '$sha' does not match expected '$ExpectedSha'." }
    return $sha
}

$commands = @(
    [ordered]@{ id = 'retrieval_recall'; argv = @('cargo','test','-p','nano-memory','--test','retrieval_recall','--','--nocapture') },
    [ordered]@{ id = 'durability'; argv = @('cargo','test','-p','nano-memory','--test','durability','--','--nocapture') },
    [ordered]@{ id = 'write_mediation'; argv = @('cargo','test','-p','nano-memory','--test','write_mediation','--','--nocapture') },
    [ordered]@{ id = 'nano_memory'; argv = @('cargo','test','-p','nano-memory') },
    [ordered]@{ id = 'nano_session'; argv = @('cargo','test','-p','nano-session') },
    [ordered]@{ id = 'fmt'; argv = @('cargo','fmt','--all','--','--check') },
    [ordered]@{ id = 'clippy'; argv = @('cargo','clippy','-p','nano-memory','-p','nano-session','--all-targets','--','-D','warnings') }
)
if ($IncludeWorkspace) {
    $commands += [ordered]@{ id = 'workspace_test'; argv = @('cargo','test','--workspace') }
}

$null = Assert-ExpectedHead
$started = [DateTime]::UtcNow
$results = @()
$oldTarget = $env:CARGO_TARGET_DIR
$env:CARGO_TARGET_DIR = 'F:\CargoTarget\wayland-nano'
try {
    foreach ($command in $commands) {
        $preSha = Assert-ExpectedHead
        $stdoutPath = [IO.Path]::GetTempFileName()
        $stderrPath = [IO.Path]::GetTempFileName()
        try {
            $process = Start-Process -FilePath $command.argv[0] `
                -ArgumentList $command.argv[1..($command.argv.Count - 1)] `
                -WorkingDirectory $repositoryPath -NoNewWindow -Wait -PassThru `
                -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
            $stdout = [IO.File]::ReadAllText($stdoutPath)
            $stderr = [IO.File]::ReadAllText($stderrPath)
        } finally {
            Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
        }
        $results += [ordered]@{
            id = $command.id
            argv = $command.argv
            working_directory = $repositoryPath
            pre_command_sha = $preSha
            exit_code = $process.ExitCode
            stdout = $stdout
            stderr = $stderr
        }
        if ($process.ExitCode -ne 0) { break }
    }
} finally {
    $env:CARGO_TARGET_DIR = $oldTarget
}

$recallOutput = ($results | Where-Object id -eq 'retrieval_recall').stdout
$durabilityOutput = ($results | Where-Object id -eq 'durability').stdout
$mediationOutput = ($results | Where-Object id -eq 'write_mediation').stdout
$recallMatch = [regex]::Match($recallOutput, 'recall@10=(?<recall>\d+(?:\.\d+)?); cross-project=(?<project>\d+); cross-agent=(?<agent>\d+)')
$metrics = [ordered]@{
    recall_at_10 = if ($recallMatch.Success) { [double]::Parse($recallMatch.Groups['recall'].Value, [Globalization.CultureInfo]::InvariantCulture) } else { $null }
    cross_project_rows = if ($recallMatch.Success) { [int]$recallMatch.Groups['project'].Value } else { $null }
    cross_agent_rows = if ($recallMatch.Success) { [int]$recallMatch.Groups['agent'].Value } else { $null }
    kill_rebuild_query_equivalent = $durabilityOutput -match 'kill-mid-write rebuild query-equivalent'
    kill_rebuild_includes_agent_id = $durabilityOutput -match 'agent_id and current facts identical'
    mediation_receipt_visible = $mediationOutput -match 'model_proposes_host_commits_and_receipts \.\.\. ok'
}

$receipt = [ordered]@{
    schema_version = 'pmem-acceptance-v1'
    repository = $repositoryPath
    tested_sha = $ExpectedSha
    started_at_utc = $started.ToString('o')
    finished_at_utc = [DateTime]::UtcNow.ToString('o')
    include_workspace = [bool]$IncludeWorkspace
    commands = $results
    metrics = $metrics
}
$parent = Split-Path -Parent $OutputPath
if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
$absoluteOutput = if ([IO.Path]::IsPathRooted($OutputPath)) { $OutputPath } else { Join-Path (Get-Location) $OutputPath }
[IO.File]::WriteAllText($absoluteOutput, ($receipt | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))

if (@($results | Where-Object exit_code -ne 0).Count -gt 0) {
    throw "Acceptance command '$($results[-1].id)' failed with exit code $($results[-1].exit_code); receipt written to $OutputPath."
}
Write-Output "Acceptance receipt written for $ExpectedSha ($($results.Count) commands)."
