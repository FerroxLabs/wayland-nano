[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$workflowPath = Join-Path $PSScriptRoot 'verify-receipt-check.yml'
$tempRoot = [System.IO.Path]::GetFullPath($env:TEMP)
if ([System.IO.Path]::GetPathRoot($tempRoot) -notmatch '^[Ff]:\\$') {
    throw "TEMP must be F:-resident for this oracle (resolved: $tempRoot)"
}

$oracleRoot = Join-Path $tempRoot ("wn-rd-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
$repo = Join-Path $oracleRoot 'r'
$stub = Join-Path $oracleRoot 'verifier-success.ps1'

function Invoke-Git {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
    & git -C $repo @Arguments
    if ($LASTEXITCODE -ne 0) { throw "git failed: git $($Arguments -join ' ')" }
}

function Invoke-ReceiptSelection {
    param([string]$Base, [string]$Head)
    $lines = @(& git -C $repo diff --name-status "$Base...$Head" -- 'receipts/**')
    if ($LASTEXITCODE -ne 0) { return 90 }
    foreach ($line in $lines) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        $fields = $line -split "`t"
        $status = $fields[0]
        switch -Wildcard ($status) {
            'D*' { return 1 }
            'R*' { return 1 }
            'A*' {
                & $stub (Join-Path $repo $fields[1])
                if ($LASTEXITCODE -ne 0) { return $LASTEXITCODE }
            }
            'M*' {
                & $stub (Join-Path $repo $fields[1])
                if ($LASTEXITCODE -ne 0) { return $LASTEXITCODE }
            }
            default { return 1 }
        }
    }
    return 0
}

function Assert-Exit {
    param([string]$Case, [int]$Expected, [int]$Actual)
    if ($Actual -ne $Expected) { throw "$Case expected exit $Expected, got $Actual" }
}

try {
    New-Item -ItemType Directory -Path (Join-Path $repo 'receipts') -Force | Out-Null
    Set-Content -LiteralPath $stub -Encoding utf8 -Value 'param([string]$Receipt); if (-not (Test-Path -LiteralPath $Receipt -PathType Leaf)) { exit 9 }; exit 0'
    Invoke-Git init --initial-branch=main
    Invoke-Git config user.email 'receipt-oracle@example.invalid'
    Invoke-Git config user.name 'Receipt Oracle'
    Set-Content -LiteralPath (Join-Path $repo 'receipts/base.json') -Encoding utf8 -Value '{"schema":1}'
    Invoke-Git add receipts/base.json
    Invoke-Git commit -m base
    $base = (& git -C $repo rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'could not resolve base commit' }

    Assert-Exit 'no changes' 0 (Invoke-ReceiptSelection $base $base)

    Invoke-Git switch -c case-add $base
    Set-Content -LiteralPath (Join-Path $repo 'receipts/added.json') -Encoding utf8 -Value '{"schema":1}'
    Invoke-Git add receipts/added.json
    Invoke-Git commit -m add
    Assert-Exit 'add' 0 (Invoke-ReceiptSelection $base 'HEAD')

    Invoke-Git switch -C case-modify $base
    Set-Content -LiteralPath (Join-Path $repo 'receipts/base.json') -Encoding utf8 -Value '{"schema":1,"changed":true}'
    Invoke-Git add receipts/base.json
    Invoke-Git commit -m modify
    Assert-Exit 'modify' 0 (Invoke-ReceiptSelection $base 'HEAD')

    Invoke-Git switch -C case-delete $base
    Invoke-Git rm receipts/base.json
    Invoke-Git commit -m delete
    Assert-Exit 'delete' 1 (Invoke-ReceiptSelection $base 'HEAD')

    Invoke-Git switch -C case-rename $base
    Invoke-Git mv receipts/base.json receipts/renamed.json
    Invoke-Git commit -m rename
    Assert-Exit 'rename' 1 (Invoke-ReceiptSelection $base 'HEAD')

    $workflow = Get-Content -Raw -LiteralPath $workflowPath
    foreach ($arm in @('D*|R*)', 'A*|M*)')) {
        if (-not $workflow.Contains($arm)) { throw "workflow selector is missing case arm: $arm" }
    }

    Write-Host 'receipt diff oracle: A/M pass; D/R fail; unchanged passes'
    exit 0
}
finally {
    if (Test-Path -LiteralPath $oracleRoot) {
        Remove-Item -LiteralPath $oracleRoot -Recurse -Force
    }
}
