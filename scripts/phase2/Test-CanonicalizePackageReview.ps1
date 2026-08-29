[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ReceiptPath,
    [Parameter(Mandatory = $true)]
    [string]$SchemaPath,
    [switch]$RequireOwnerDecision
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-Equal {
    param([object]$Actual, [object]$Expected, [string]$Label)
    if ([string]$Actual -cne [string]$Expected) {
        throw "$Label mismatch: expected '$Expected', got '$Actual'"
    }
}

function Assert-True {
    param([bool]$Condition, [string]$Label)
    if (-not $Condition) { throw "$Label failed" }
}

function Get-LowerHash {
    param([string]$Path, [string]$Algorithm = 'SHA256')
    return (Get-FileHash -LiteralPath $Path -Algorithm $Algorithm).Hash.ToLowerInvariant()
}

function Get-ObjectKeys {
    param([object]$Object)
    return @($Object.PSObject.Properties | ForEach-Object { $_.Name } | Sort-Object)
}

function Assert-ExactKeys {
    param([object]$Object, [string[]]$Expected, [string]$Label)
    $actual = @(Get-ObjectKeys $Object)
    $wanted = @($Expected | Sort-Object)
    Assert-Equal ($actual -join ',') ($wanted -join ',') "$Label keys"
}

$receiptFile = (Resolve-Path -LiteralPath $ReceiptPath).Path
$schemaFile = (Resolve-Path -LiteralPath $SchemaPath).Path
$receipt = Get-Content -LiteralPath $receiptFile -Raw | ConvertFrom-Json
$schema = Get-Content -LiteralPath $schemaFile -Raw | ConvertFrom-Json

Assert-Equal $schema.'$schema' 'https://json-schema.org/draft/2020-12/schema' 'schema dialect'
Assert-True ($schema.additionalProperties -eq $false) 'closed receipt schema'
Assert-ExactKeys $receipt @('schema','package','registry','source','contents','runtime','vectors','decision') 'receipt'
Assert-Equal $receipt.schema 'wayland.nano.canonicalize-package-review/v1' 'receipt schema'
Assert-Equal $receipt.package.name 'canonicalize' 'package name'
Assert-Equal $receipt.package.version '2.1.0' 'package version'

$auditRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("wayland-nano-canonicalize-review-" + [guid]::NewGuid().ToString('N'))
$tarball = Join-Path $auditRoot 'canonicalize-2.1.0.tgz'
$sourceZip = Join-Path $auditRoot 'source.zip'
$packageRoot = Join-Path $auditRoot 'package'
$sourceRoot = Join-Path $auditRoot 'source'
New-Item -ItemType Directory -Path $auditRoot | Out-Null

try {
    $metadata = Invoke-RestMethod -Uri 'https://registry.npmjs.org/canonicalize/2.1.0' -Method Get
    $packument = Invoke-RestMethod -Uri 'https://registry.npmjs.org/canonicalize' -Method Get
    Assert-Equal $metadata.name 'canonicalize' 'live metadata name'
    Assert-Equal $metadata.version '2.1.0' 'live metadata version'
    Assert-Equal $metadata.dist.tarball $receipt.registry.tarball_url 'live tarball URL'
    Assert-Equal $metadata.dist.integrity $receipt.registry.dist_integrity 'live dist integrity'
    Assert-Equal $metadata.dist.shasum $receipt.registry.dist_shasum 'live dist shasum'
    Assert-Equal $metadata.license $receipt.package.license 'live license'
    Assert-Equal $packument.time.'2.1.0' $receipt.registry.published_at 'published time'
    Assert-Equal (@($metadata.maintainers | ForEach-Object { "{0} <{1}>" -f $_.name, $_.email }) -join '|') ($receipt.registry.maintainers -join '|') 'maintainers'

    Invoke-WebRequest -UseBasicParsing -Uri $receipt.registry.tarball_url -OutFile $tarball
    Assert-Equal (Get-LowerHash $tarball 'SHA256') $receipt.registry.tarball_sha256 'tarball SHA-256'
    Assert-Equal (Get-LowerHash $tarball 'SHA1') $receipt.registry.dist_shasum 'tarball SHA-1'
    $sha512Provider = [Security.Cryptography.SHA512]::Create()
    try {
        $sha512 = [Convert]::ToBase64String($sha512Provider.ComputeHash([IO.File]::ReadAllBytes($tarball)))
    }
    finally {
        $sha512Provider.Dispose()
    }
    Assert-Equal ("sha512-" + $sha512) $receipt.registry.dist_integrity 'tarball SRI'

    $tarExe = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (-not (Test-Path -LiteralPath $tarExe)) { throw 'Windows tar.exe is required' }
    $entries = @(& $tarExe -tf $tarball)
    if ($LASTEXITCODE -ne 0) { throw 'tar list failed' }
    Assert-True ($entries.Count -eq 20) 'tar entry count'
    foreach ($entry in $entries) {
        Assert-True ($entry -match '^package/[A-Za-z0-9._/-]+$') "safe tar entry $entry"
        Assert-True (-not $entry.Contains('../')) "no traversal in $entry"
    }
    & $tarExe -xzf $tarball -C $auditRoot
    if ($LASTEXITCODE -ne 0) { throw 'tar extract failed' }

    $files = @(Get-ChildItem -LiteralPath $packageRoot -Recurse -File | ForEach-Object {
        [pscustomobject]@{
            path = $_.FullName.Substring($packageRoot.Length + 1).Replace('\','/')
            sha256 = Get-LowerHash $_.FullName
            size = $_.Length
        }
    } | Sort-Object path)
    Assert-Equal $files.Count $receipt.contents.file_count 'file count'
    $recordedFiles = @($receipt.contents.files | Sort-Object path)
    for ($index = 0; $index -lt $files.Count; $index++) {
        Assert-Equal $files[$index].path $recordedFiles[$index].path "file[$index] path"
        Assert-Equal $files[$index].sha256 $recordedFiles[$index].sha256 "file[$index] hash"
        Assert-Equal $files[$index].size $recordedFiles[$index].size "file[$index] size"
    }

    $manifestPath = Join-Path $packageRoot 'package.json'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    Assert-Equal (Get-LowerHash $manifestPath) $receipt.package.manifest_sha256 'manifest SHA-256'
    foreach ($field in @('name','version','description','main','types','bin','license')) {
        Assert-Equal $manifest.$field $receipt.package.manifest.$field "manifest $field"
    }
    Assert-Equal $manifest.repository.url $receipt.package.manifest.repository.url 'manifest repository URL'
    Assert-True (-not ($manifest.PSObject.Properties.Name -contains 'dependencies')) 'no runtime dependencies'
    Assert-True (-not ($manifest.PSObject.Properties.Name -contains 'optionalDependencies')) 'no optional dependencies'
    Assert-True (-not ($manifest.PSObject.Properties.Name -contains 'peerDependencies')) 'no peer dependencies'
    $lifecycleNames = @('preinstall','install','postinstall','prepublish','prepare','prepack','postpack')
    $presentLifecycle = @($lifecycleNames | Where-Object { $manifest.scripts.PSObject.Properties.Name -contains $_ })
    Assert-Equal $presentLifecycle.Count 0 'lifecycle script count'
    Assert-Equal (@(Get-ObjectKeys $receipt.contents.dependencies).Count) 0 'recorded dependencies'
    Assert-Equal (@(Get-ObjectKeys $receipt.contents.lifecycle_scripts).Count) 0 'recorded lifecycle scripts'

    $tag = Invoke-RestMethod -Uri 'https://api.github.com/repos/erdtman/canonicalize/git/ref/tags/v2.1.0' -Headers @{ 'User-Agent' = 'wayland-nano-package-audit' }
    Assert-Equal $tag.object.sha $receipt.source.tag_object_sha 'tag object SHA'
    $tagObject = Invoke-RestMethod -Uri $tag.object.url -Headers @{ 'User-Agent' = 'wayland-nano-package-audit' }
    Assert-Equal $tagObject.object.sha $receipt.source.commit_sha 'tag commit SHA'
    Assert-Equal ([bool]$tagObject.verification.verified) ([bool]$receipt.source.tag_signature_verified) 'tag signature state'
    $commit = Invoke-RestMethod -Uri ("https://api.github.com/repos/erdtman/canonicalize/git/commits/" + $receipt.source.commit_sha) -Headers @{ 'User-Agent' = 'wayland-nano-package-audit' }
    Assert-Equal $commit.tree.sha $receipt.source.tree_sha 'source tree SHA'
    Invoke-WebRequest -UseBasicParsing -Uri ("https://codeload.github.com/erdtman/canonicalize/zip/" + $receipt.source.commit_sha) -OutFile $sourceZip
    Expand-Archive -LiteralPath $sourceZip -DestinationPath $sourceRoot
    $sourceCheckout = (Get-ChildItem -LiteralPath $sourceRoot -Directory | Select-Object -First 1).FullName
    foreach ($file in $files) {
        $sourceFile = Join-Path $sourceCheckout $file.path
        Assert-True (Test-Path -LiteralPath $sourceFile -PathType Leaf) "source contains $($file.path)"
        Assert-Equal (Get-LowerHash $sourceFile) $file.sha256 "source correspondence $($file.path)"
    }

    $nanoRoot = 'D:\Development\waylandnano\wayland-nano\.tmp-wt-phase2'
    $vectorsPath = Join-Path $nanoRoot 'contracts\activation\vectors\positive.json'
    $vectorsManifestPath = Join-Path $nanoRoot 'contracts\activation\vectors\manifest.json'
    Assert-True (Test-Path -LiteralPath $vectorsPath) 'Nano positive vectors present'
    Assert-Equal (Get-LowerHash $vectorsManifestPath) $receipt.vectors.nano_manifest_sha256 'Nano vector manifest SHA-256'

    $runnerPath = Join-Path $auditRoot 'verify-runtime.cjs'
    $runner = @'
const fs = require('fs');
const crypto = require('crypto');
const canonicalize = require(process.argv[2]);
const vectors = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
if (typeof canonicalize !== 'function') throw new Error('CommonJS export is not a function');
const unsigned = value => { const copy = JSON.parse(JSON.stringify(value)); delete copy.signature; return copy; };
const exactUnsigned = raw => raw.replace(/,"signature":"[^"]+"(?=})/, '').replace(/"signature":"[^"]+",/, '');
const cases = {
  activation: vectors.activation.raw_frame_utf8,
  receipt: vectors.receipt.raw_receipt_utf8,
  control: vectors.control.raw_control_utf8,
  admin: vectors.admin.raw_admin_utf8,
};
const subjects = {
  activation: JSON.parse(cases.activation).params._meta.waylandNanoActivation,
  receipt: JSON.parse(cases.receipt),
  control: JSON.parse(cases.control),
  admin: JSON.parse(cases.admin),
};
for (const name of Object.keys(subjects)) {
  const actual = canonicalize(unsigned(subjects[name]));
  const expected = exactUnsigned(name === 'activation' ? JSON.stringify(subjects[name]) : cases[name]);
  if (actual !== expected) throw new Error(`${name} canonical bytes mismatch`);
}
const activationHash = crypto.createHash('sha256').update(canonicalize(unsigned(subjects.activation))).digest('hex');
if (activationHash !== vectors.activation.canonical_payload_sha256) throw new Error('activation hash mismatch');
const rfcInput = {'\u20ac':'Euro Sign','\r':'Carriage Return','\ufb33':'Hebrew Letter Dalet With Dagesh','1':'One','\ud83d\ude00':'Emoji: Grinning Face','\u0080':'Control','\u00f6':'Latin Small Letter O With Diaeresis'};
const rfcExpected = '{"\\r":"Carriage Return","1":"One","\u0080":"Control","\u00f6":"Latin Small Letter O With Diaeresis","\u20ac":"Euro Sign","\ud83d\ude00":"Emoji: Grinning Face","\ufb33":"Hebrew Letter Dalet With Dagesh"}';
if (canonicalize(rfcInput) !== rfcExpected) throw new Error('RFC 8785 UTF-16 property ordering mismatch');
process.stdout.write(JSON.stringify({exportType: typeof canonicalize, rfc8785: true, activation: true, receipt: true, control: true, admin: true}));
'@
    [IO.File]::WriteAllText($runnerPath, $runner, [Text.UTF8Encoding]::new($false))
    $modulePath = Join-Path $packageRoot 'lib\canonicalize.js'
    $nodeOutput = & node $runnerPath $modulePath $vectorsPath
    if ($LASTEXITCODE -ne 0) { throw 'Node 24 export/vector verification failed' }
    $bunOutput = & bun $runnerPath $modulePath $vectorsPath
    if ($LASTEXITCODE -ne 0) { throw 'Bun export/vector verification failed' }
    $nodeResult = $nodeOutput | ConvertFrom-Json
    $bunResult = $bunOutput | ConvertFrom-Json
    Assert-Equal $nodeResult.exportType $receipt.runtime.commonjs_export 'Node export shape'
    Assert-Equal $bunResult.exportType $receipt.runtime.commonjs_export 'Bun export shape'
    foreach ($flag in @('rfc8785','activation','receipt','control','admin')) {
        Assert-True ([bool]$nodeResult.$flag) "Node $flag vector"
        Assert-True ([bool]$bunResult.$flag) "Bun $flag vector"
    }
    Assert-True ([bool]$receipt.runtime.node_pass) 'recorded Node result'
    Assert-True ([bool]$receipt.runtime.bun_pass) 'recorded Bun result'
    Assert-True ([bool]$receipt.vectors.rfc8785_pass) 'recorded RFC result'
    Assert-True ([bool]$receipt.vectors.nano_activation_pass) 'recorded activation result'
    Assert-True ([bool]$receipt.vectors.nano_receipt_pass) 'recorded receipt result'
    Assert-True ([bool]$receipt.vectors.nano_control_pass) 'recorded control result'
    Assert-True ([bool]$receipt.vectors.nano_admin_pass) 'recorded admin result'

    if ($RequireOwnerDecision) {
        Assert-Equal $receipt.decision.outcome 'approved_exact_package' 'owner decision outcome'
        Assert-Equal $receipt.decision.approved_name $receipt.package.name 'approved name'
        Assert-Equal $receipt.decision.approved_version $receipt.package.version 'approved version'
        Assert-Equal $receipt.decision.approved_integrity $receipt.registry.dist_integrity 'approved integrity'
        Assert-Equal $receipt.decision.approved_tarball_sha256 $receipt.registry.tarball_sha256 'approved tarball SHA-256'
        Assert-Equal $receipt.decision.authority_mode 'owner-directed-agent-operated' 'authority disclosure'
        Assert-True ([bool]$receipt.decision.same_human_controller) 'same-human-controller disclosure'
        Assert-True (-not [bool]$receipt.decision.independent_human_review) 'no independent-human-review claim'
    }

    Write-Output 'PASS canonicalize@2.1.0 exact artifact, source correspondence, exports, and vectors verified'
}
finally {
    if ((Test-Path -LiteralPath $auditRoot) -and $auditRoot.StartsWith([System.IO.Path]::GetTempPath(), [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $auditRoot -Recurse -Force
    }
}
