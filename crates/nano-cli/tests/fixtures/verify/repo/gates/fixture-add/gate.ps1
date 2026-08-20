param([string]$Artifact)
$text = [System.IO.File]::ReadAllText($Artifact)
if ($text.Contains('a + b') -or $text.StartsWith('diff --git ')) {
  Write-Output 'fixture-add-gate: 1/1'
  exit 0
}
Write-Output 'FAIL FX-01 value'
Write-Output 'fixture-add-gate: 0/1'
exit 7
