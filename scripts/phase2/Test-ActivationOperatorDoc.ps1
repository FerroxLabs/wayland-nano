param(
    [Parameter(Mandatory=$true)][string]$Path,
    [switch]$RequireAllHeadings,
    [switch]$RequireExecutableCommandShapes,
    [switch]$RequireDefaultOff,
    [switch]$RejectSecretExamples
)
$ErrorActionPreference = 'Stop'
$text = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $Path))
$headings = @('Receipt signer rotation','Verifier rotation and distribution','Retention','Revocation','Compromise recovery','Rollback and default-off','Platform key references','Offline verification','No-secret rules')
$families = 'admin|activation|receipt'
foreach ($heading in $headings) {
    $matches = [regex]::Matches($text, "(?m)^## $([regex]::Escape($heading))$")
    if ($RequireAllHeadings -and $matches.Count -ne 1) { throw "heading '$heading' must occur exactly once" }
    $section = [regex]::Match($text, "(?ms)^## $([regex]::Escape($heading))\r?\n(?<body>.*?)(?=^## |\z)").Groups['body'].Value
    if ($RequireExecutableCommandShapes -and $section -notmatch "(?m)^``wayland-nano ($families) [a-z0-9-]+(?: [^`\r\n]+)?``$") { throw "heading '$heading' lacks allowlisted wayland-nano argv" }
    if ($RequireExecutableCommandShapes -and $section -notmatch '(?m)^Expected: .+') { throw "heading '$heading' lacks expected receipt/effect" }
}
if ($RequireDefaultOff) {
    foreach ($required in @('default-off','keeps activation disabled','Missing recovery authority leaves activation disabled')) { if ($text -notmatch [regex]::Escape($required)) { throw "missing default-off contract: $required" } }
}
if ($RejectSecretExamples) {
    foreach ($forbidden in @('\$env:','\$\{','--private-key','BEGIN PRIVATE KEY','NANO_.*_KEY=')) { if ($text -match $forbidden) { throw "forbidden secret/interpolation example: $forbidden" } }
}
Write-Output 'activation operator document contract: PASS'
