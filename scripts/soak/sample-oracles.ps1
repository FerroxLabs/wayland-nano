param([Parameter(Mandatory=$true)][int]$PidValue,[Parameter(Mandatory=$true)][string]$NanoHome)
$ErrorActionPreference = 'Stop'
$process = Get-Process -Id $PidValue
$all = Get-CimInstance Win32_Process
$descendants = @()
$frontier = @($PidValue)
while ($frontier.Count -gt 0) {
  $parents = $frontier
  $children = @($all | Where-Object { $parents -contains [int]$_.ParentProcessId })
  $frontier = @($children | ForEach-Object { [int]$_.ProcessId })
  $descendants += $frontier
}
$homeBytes = 0
if (Test-Path -LiteralPath $NanoHome) {
  $homeBytes = [long](Get-ChildItem -LiteralPath $NanoHome -File -Recurse -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
}
[ordered]@{
  at = [DateTime]::UtcNow.ToString('o'); pid = $PidValue
  privateWorkingSetBytes = [long]$process.PrivateMemorySize64
  workingSetBytes = [long]$process.WorkingSet64
  handles = [int]$process.HandleCount; threads = [int]$process.Threads.Count
  openFds = $null; nanoHomeBytes = $homeBytes; descendants = $descendants
} | ConvertTo-Json -Compress
