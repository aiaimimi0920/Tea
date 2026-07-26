[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDir,

    [Parameter(Mandatory = $true)]
    [string]$Tag,

    [string]$OutputDir = "",
    [switch]$Force,
    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$resolvedPackageDir = [System.IO.Path]::GetFullPath($PackageDir)
$resolvedOutputDir = if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    Join-Path $resolvedPackageDir "packages"
} elseif ([System.IO.Path]::IsPathRooted($OutputDir)) {
    [System.IO.Path]::GetFullPath($OutputDir)
} else {
    [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputDir))
}
$assetName = "tea-windows-x64-$Tag.zip"
$zipPath = Join-Path $resolvedOutputDir $assetName

if ($DryRun) {
    [ordered]@{
        packageDir = $resolvedPackageDir
        outputDir = $resolvedOutputDir
        assetName = $assetName
        zipPath = $zipPath
    } | ConvertTo-Json -Depth 5
    exit 0
}

if (-not (Test-Path -LiteralPath $resolvedPackageDir -PathType Container)) {
    throw "Missing Tea package directory: $resolvedPackageDir"
}
if (-not (Test-Path -LiteralPath (Join-Path $resolvedPackageDir "manifest.json") -PathType Leaf)) {
    throw "Tea package directory must contain manifest.json: $resolvedPackageDir"
}
if (-not (Test-Path -LiteralPath $resolvedOutputDir)) {
    New-Item -ItemType Directory -Path $resolvedOutputDir -Force | Out-Null
}
if ((Test-Path -LiteralPath $zipPath -PathType Leaf) -and -not $Force) {
    throw "Release zip already exists. Re-run with -Force to replace it: $zipPath"
}
if (Test-Path -LiteralPath $zipPath -PathType Leaf) {
    Remove-Item -LiteralPath $zipPath -Force
}

$stagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("tea-release-" + [System.Guid]::NewGuid().ToString("N"))
try {
    New-Item -ItemType Directory -Path $stagingRoot -Force | Out-Null
    Get-ChildItem -LiteralPath $resolvedPackageDir -Force | Where-Object { $_.Name -ne "packages" } | ForEach-Object {
        $target = Join-Path $stagingRoot $_.Name
        if ($_.PSIsContainer) {
            Copy-Item -LiteralPath $_.FullName -Destination $target -Recurse -Force
        } else {
            Copy-Item -LiteralPath $_.FullName -Destination $target -Force
        }
    }
    Compress-Archive -Path (Join-Path $stagingRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal
} finally {
    if (Test-Path -LiteralPath $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force
    }
}

$hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
$shaPath = "$zipPath.sha256"
[System.IO.File]::WriteAllText($shaPath, "$hash  $assetName`r`n", [System.Text.ASCIIEncoding]::new())

Write-Host "[tea-release-package] Created:"
Write-Host "  $zipPath"