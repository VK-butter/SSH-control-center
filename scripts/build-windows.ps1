#requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$Desktop,
    [switch]$SkipBuild,
    [switch]$RefreshStage,
    [switch]$ThinTemplateOnly,
    [string]$TemplatesDir,
    [string]$NsisPath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$manifestText=[IO.File]::ReadAllText((Join-Path $root 'Cargo.toml'))
$versionMatch=[regex]::Match($manifestText,'(?m)^version\s*=\s*"([^"]+)"')
if(-not $versionMatch.Success){throw 'Cargo workspace version is missing.'}
$version=$versionMatch.Groups[1].Value
$artifacts = Join-Path $root 'dist'
$stage = Join-Path $artifacts ('sshcc-'+$version+'-windows-x64')
$release = if ($env:CARGO_TARGET_DIR) { Join-Path $env:CARGO_TARGET_DIR 'release' } else { Join-Path $root 'target\release' }
function Invoke-Checked([string]$Program,[string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
Push-Location $root
try {
    if (-not $SkipBuild) {
        Invoke-Checked cargo @('build','--release','--locked','-p','sshcc')
        if ($Desktop) { Invoke-Checked cargo @('build','--release','--locked','-p','sshcc-desktop') }
    }
    if (Test-Path -LiteralPath $stage) {
        if (-not $RefreshStage) { throw "Staging already exists: $stage. Use -RefreshStage to rebuild these owned package artifacts." }
        $allowed = @('sshcc.exe','sshcc-desktop.exe','sshcc-window.exe','Setup.ps1','Uninstall.ps1','PairingBootstrap.ps1','Configure-Codex.ps1','INSTALL.md','templates')
        $unexpected = @(Get-ChildItem -LiteralPath $stage -Force | Where-Object { $_.Name -notin $allowed -or ($_.PSIsContainer -and $_.Name -ne 'templates') })
        if ($unexpected.Count) { throw 'Refusing to package staging with unexpected files.' }
        if (-not $Desktop -and (Test-Path -LiteralPath (Join-Path $stage 'sshcc-desktop.exe'))) { throw 'Use a fresh staging directory when changing desktop package flavor.' }
    }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $release 'sshcc.exe') -Destination $stage
    if ($Desktop) {
        Copy-Item -LiteralPath (Join-Path $release 'sshcc-desktop.exe') -Destination $stage
        Copy-Item -LiteralPath (Join-Path $release 'sshcc-window.exe') -Destination $stage
    }
    foreach ($name in @('Setup.ps1','Uninstall.ps1','PairingBootstrap.ps1','Configure-Codex.ps1')) {
        Copy-Item -LiteralPath (Join-Path $root ('installers\windows\'+$name)) -Destination $stage
    }
    Copy-Item -LiteralPath (Join-Path $root 'docs\INSTALL.md') -Destination (Join-Path $stage 'INSTALL.md')
    if (-not $NsisPath) {
        $nsis=Get-Command makensis.exe -ErrorAction SilentlyContinue
        if($nsis){$NsisPath=$nsis.Source}
    }
    if(-not $NsisPath -or -not (Test-Path -LiteralPath $NsisPath -PathType Leaf)){throw 'NSIS compiler is required to build the single-file client template. Pass -NsisPath.'}
    # Thin templates never embed other templates, preventing packaging cycles.
    $thin=Join-Path $artifacts 'sshcc-client-windows-template.exe'
    if ($ThinTemplateOnly -or -not $TemplatesDir) {
        Invoke-Checked $NsisPath @(('/DPACKAGE_DIR='+$stage),('/DOUTPUT_FILE='+$thin),('/DVERSION='+$version),'/DREQUIRE_PAIRING=1',(Join-Path $root 'installers\windows\sshcc.nsi'))
    }
    if(-not $ThinTemplateOnly){
        if(-not $TemplatesDir){throw 'Full Master release requires -TemplatesDir with both OS templates and template-manifest.json. Thin Windows template was built successfully.'}
        $templates=Join-Path $stage 'templates'
        New-Item -ItemType Directory -Path $templates -Force | Out-Null
        foreach($name in @('sshcc-client-windows-template.exe','sshcc-client-ubuntu-template.deb','template-manifest.json')){
            $source=Join-Path $TemplatesDir $name
            if(-not (Test-Path -LiteralPath $source -PathType Leaf)){throw "Full release is missing $name"}
            if([IO.Path]::GetFullPath($source) -ne [IO.Path]::GetFullPath((Join-Path $templates $name))){
                Copy-Item -LiteralPath $source -Destination (Join-Path $templates $name) -Force
            }
        }
        $templateManifest=Get-Content -LiteralPath (Join-Path $templates 'template-manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
        if($templateManifest.schemaVersion -ne 1 -or @($templateManifest.templates).Count -ne 2){throw 'Template manifest must include exactly both supported OS assets.'}
        foreach($entry in $templateManifest.templates){
            $expectedName=if($entry.os -eq 'windows'){'sshcc-client-windows-template.exe'}elseif($entry.os -eq 'linux'){'sshcc-client-ubuntu-template.deb'}else{throw 'Unknown template OS'}
            if($entry.fileName -ne $expectedName -or $entry.sha256 -notmatch '^[0-9a-f]{64}$'){throw 'Invalid template manifest entry.'}
            $actual=(Get-FileHash -LiteralPath (Join-Path $templates $expectedName) -Algorithm SHA256).Hash.ToLowerInvariant()
            if($actual -ne $entry.sha256){throw "Template checksum mismatch: $expectedName"}
        }
        $zip = Join-Path $artifacts ('sshcc-'+$version+'-windows-x64.zip')
        Compress-Archive -LiteralPath $stage -DestinationPath $zip -Force
        $setup = Join-Path $artifacts ('sshcc-'+$version+'-windows-x64-setup.exe')
        Invoke-Checked $NsisPath @(('/DPACKAGE_DIR='+$stage),('/DOUTPUT_FILE='+$setup),('/DVERSION='+$version),'/DEMBED_TEMPLATES=1',(Join-Path $root 'installers\windows\sshcc.nsi'))
    }
    $hashes = Get-ChildItem -LiteralPath $artifacts -File | Where-Object { $_.Extension -in @('.zip','.exe','.deb') } | Sort-Object Name | ForEach-Object {
        (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()+'  '+$_.Name
    }
    [IO.File]::WriteAllLines((Join-Path $artifacts 'SHA256SUMS'),$hashes,(New-Object Text.UTF8Encoding($false)))
    Write-Host "Artifacts: $artifacts"
} finally { Pop-Location }
