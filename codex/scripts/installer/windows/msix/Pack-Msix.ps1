# Pack an unsigned store-scaffold MSIX from the existing cargo staging dir.
# Does not touch NSIS / portable ZIP outputs. Does not enable Tauri bundle.
#
# Partner Center 还没批下来，下列字段先占位（见 AppxManifest.xml.template）：
#   Identity.Name
#   Identity.Publisher
#   PublisherDisplayName
#
# Usage (from codex/ after staging dist/windows/app):
#   ./scripts/installer/windows/msix/Pack-Msix.ps1 -PackageVersion $env:PACKAGE_VERSION

[CmdletBinding()]
param(
    [string]$PackageVersion,
    [string]$AppDir,
    [string]$OutDir,
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function ConvertTo-MsixIdentityVersion {
    param([Parameter(Mandatory = $true)][string]$PackageVersion)

    $mapper = Join-Path $PSScriptRoot 'map-package-version.mjs'
    if (-not (Test-Path -LiteralPath $mapper)) {
        throw "Missing version mapper: $mapper"
    }
    $node = Get-Command node -ErrorAction SilentlyContinue
    if (-not $node) {
        throw "node is required to map PACKAGE_VERSION (Windows CI already installs Node 22)."
    }
    $mapped = & node $mapper $PackageVersion
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to map PACKAGE_VERSION '$PackageVersion' to MSIX x.y.z.w"
    }
    return ([string]$mapped).Trim()
}

function Find-MakeAppx {
    $patterns = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin\*\x64\makeappx.exe'),
        (Join-Path $env:ProgramFiles 'Windows Kits\10\bin\*\x64\makeappx.exe')
    )
    $hits = @()
    foreach ($pattern in $patterns) {
        $hits += @(Get-Item -Path $pattern -ErrorAction SilentlyContinue)
    }
    if ($hits.Count -eq 0) {
        throw @"
makeappx.exe not found. Looked for:
  %ProgramFiles(x86)%\Windows Kits\10\bin\*\x64\makeappx.exe
  %ProgramFiles%\Windows Kits\10\bin\*\x64\makeappx.exe
Install the Windows 10/11 SDK (MakeAppx) on this machine. GitHub windows-latest includes it.
"@
    }
    return ($hits | Sort-Object -Property FullName -Descending | Select-Object -First 1).FullName
}

if ($SelfTest) {
    $cases = @{
        '1.2.46-internal-38' = '1.2.46.38'
        '1.2.46' = '1.2.46.0'
        '1.2.46-internal' = '1.2.46.0'
        '1.2.46.7' = '1.2.46.7'
    }
    foreach ($entry in $cases.GetEnumerator()) {
        $got = ConvertTo-MsixIdentityVersion $entry.Key
        if ($got -ne $entry.Value) {
            throw "SelfTest failed: $($entry.Key) -> $got (expected $($entry.Value))"
        }
    }
    Write-Host 'Pack-Msix self-test passed'
    return
}

if (-not $PackageVersion) {
    throw '-PackageVersion is required unless -SelfTest'
}

$codexRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..\..')).Path
if (-not $AppDir) {
    $AppDir = Join-Path $codexRoot 'dist\windows\app'
}
if (-not $OutDir) {
    $OutDir = Join-Path $codexRoot 'dist\windows'
}

$AppDir = [System.IO.Path]::GetFullPath($AppDir)
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force $OutDir | Out-Null

$msixVersion = ConvertTo-MsixIdentityVersion $PackageVersion
$templatePath = Join-Path $PSScriptRoot 'AppxManifest.xml.template'
if (-not (Test-Path -LiteralPath $templatePath)) {
    throw "Missing manifest template: $templatePath"
}

$stage = Join-Path $OutDir 'msix-stage'
if (Test-Path -LiteralPath $stage) {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
New-Item -ItemType Directory -Force $stage | Out-Null

foreach ($name in @('lumio-codex.exe', 'lumio-codex-launcher.exe')) {
    $src = Join-Path $AppDir $name
    if (-not (Test-Path -LiteralPath $src)) {
        throw "Missing staged binary: $src"
    }
    Copy-Item -LiteralPath $src -Destination (Join-Path $stage $name)
}

$iconRoot = Join-Path $codexRoot 'apps\codex-plus-manager\src-tauri\icons'
$assets = Join-Path $stage 'Assets'
New-Item -ItemType Directory -Force $assets | Out-Null
foreach ($logo in @(
        'StoreLogo.png',
        'Square44x44Logo.png',
        'Square150x150Logo.png',
        'Square71x71Logo.png',
        'Square310x310Logo.png',
        'Wide310x150Logo.png'
    )) {
    $src = Join-Path $iconRoot $logo
    if (-not (Test-Path -LiteralPath $src)) {
        throw "Missing MSIX logo: $src"
    }
    Copy-Item -LiteralPath $src -Destination (Join-Path $assets $logo)
}

$template = [System.IO.File]::ReadAllText($templatePath)
$manifest = $template.Replace('__MSIX_VERSION__', $msixVersion)
$manifestPath = Join-Path $stage 'AppxManifest.xml'
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText($manifestPath, $manifest, $utf8NoBom)

$makeappx = Find-MakeAppx
$msixPath = Join-Path $OutDir "LumioCodex-$PackageVersion-windows-x64-store-unsigned.msix"
Write-Host "Identity.Version=$msixVersion"
Write-Host "makeappx=$makeappx"
Write-Host "payload=$stage"
Write-Host "output=$msixPath"

& $makeappx pack /d $stage /p $msixPath /o
if ($LASTEXITCODE -ne 0) {
    throw "makeappx pack failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $msixPath)) {
    throw "makeappx did not produce $msixPath"
}

Write-Host "Wrote $msixPath"
