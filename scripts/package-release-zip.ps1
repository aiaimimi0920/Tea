[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDir,

    [Parameter(Mandatory = $true)]
    [string]$Tag,

    [string]$OutputDir = "",
    [switch]$Force,
    [switch]$DryRun,
    [switch]$AllowDirtyManifest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Assert-NoReparsePointsBetween {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RootPath,

        [Parameter(Mandatory = $true)]
        [string]$TargetPath,

        [Parameter(Mandatory = $true)]
        [string]$Context
    )

    $rootFull = [System.IO.Path]::GetFullPath($RootPath).TrimEnd("\", "/")
    $targetFull = [System.IO.Path]::GetFullPath($TargetPath)
    $rootPrefix = $rootFull + [System.IO.Path]::DirectorySeparatorChar
    if (-not [string]::Equals($targetFull, $rootFull, [System.StringComparison]::OrdinalIgnoreCase) -and
        -not $targetFull.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Context escapes its trusted root: $targetFull"
    }

    $current = $rootFull
    $components = @($rootFull)
    if (-not [string]::Equals($targetFull, $rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        $relative = $targetFull.Substring($rootPrefix.Length)
        foreach ($segment in [System.Text.RegularExpressions.Regex]::Split($relative, '[\\/]')) {
            if ([string]::IsNullOrEmpty($segment)) { continue }
            $current = Join-Path $current $segment
            $components += $current
        }
    }

    foreach ($component in $components) {
        if (-not (Test-Path -LiteralPath $component)) { continue }
        $item = Get-Item -Force -LiteralPath $component
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$Context must not traverse a symbolic link, junction, or other reparse point: $($item.FullName)"
        }
    }
}

if ($Tag -notmatch '^V\d+\.\d+\.\d+$') {
    throw "Tea release Tag must match Vx.x.x: $Tag"
}
$resolvedPackageDir = (Resolve-Path -LiteralPath $PackageDir).Path
if (-not (Test-Path -LiteralPath $resolvedPackageDir -PathType Container)) {
    throw "Missing Tea package directory: $resolvedPackageDir"
}
$manifestPath = Join-Path $resolvedPackageDir "manifest.json"
$checksumsPath = Join-Path $resolvedPackageDir "checksums.sha256"
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Tea package directory must contain manifest.json: $resolvedPackageDir"
}
if (-not (Test-Path -LiteralPath $checksumsPath -PathType Leaf)) {
    throw "Tea package directory must contain checksums.sha256: $resolvedPackageDir"
}
$verifyScript = Join-Path $PSScriptRoot "verify-tea-release-package.ps1"
$verifyArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $verifyScript, "-PackageDir", $resolvedPackageDir)
if ($AllowDirtyManifest) { $verifyArgs += "-AllowDirtyManifest" }
$verifyOutput = @(& powershell.exe @verifyArgs 2>&1)
if ($LASTEXITCODE -ne 0) {
    $verifyOutput | ForEach-Object { Write-Output $_ }
    throw "Tea package verification failed before release asset packaging."
}
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$zipRecords = @($manifest.artifacts | Where-Object { [string]$_.kind -eq "zip" })
if ($zipRecords.Count -ne 1) {
    throw "Tea manifest must contain exactly one zip artifact. Found: $($zipRecords.Count)"
}
$zipRelative = [string]$zipRecords[0].path
if ([System.IO.Path]::IsPathRooted($zipRelative)) {
    throw "Tea manifest zip artifact path must be relative: $zipRelative"
}
$sourceZip = [System.IO.Path]::GetFullPath((Join-Path $resolvedPackageDir $zipRelative))
$packagePrefix = $resolvedPackageDir.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
if (-not $sourceZip.StartsWith($packagePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Tea manifest zip artifact escapes package directory: $zipRelative"
}
$sourceSha = "$sourceZip.sha256"
if (-not (Test-Path -LiteralPath $sourceZip -PathType Leaf) -or -not (Test-Path -LiteralPath $sourceSha -PathType Leaf)) {
    throw "Tea manifest zip artifact or sidecar is missing: $zipRelative"
}
$resolvedOutputDir = if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    Join-Path $resolvedPackageDir "packages"
} elseif ([System.IO.Path]::IsPathRooted($OutputDir)) {
    [System.IO.Path]::GetFullPath($OutputDir)
} else {
    [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputDir))
}
$releaseRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $resolvedPackageDir))
$releasePrefix = $releaseRoot.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
if (-not [string]::Equals($resolvedOutputDir, $releaseRoot, [System.StringComparison]::OrdinalIgnoreCase) -and
    -not $resolvedOutputDir.StartsWith($releasePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Tea release asset output must stay under the package release root: $releaseRoot"
}
$assetName = "tea-windows-x64-$Tag.zip"
$zipPath = Join-Path $resolvedOutputDir $assetName
$shaPath = "$zipPath.sha256"
Assert-NoReparsePointsBetween -RootPath $releaseRoot -TargetPath $resolvedOutputDir -Context "Tea release asset output"
Assert-NoReparsePointsBetween -RootPath $releaseRoot -TargetPath $zipPath -Context "Tea release ZIP path"
Assert-NoReparsePointsBetween -RootPath $releaseRoot -TargetPath $shaPath -Context "Tea release sidecar path"

if ($DryRun) {
    [ordered]@{
        packageDir = $resolvedPackageDir
        outputDir = $resolvedOutputDir
        assetName = $assetName
        zipPath = $zipPath
        shaPath = $shaPath
        sourceZip = $sourceZip
    } | ConvertTo-Json -Depth 5
    exit 0
}

if (-not (Test-Path -LiteralPath $resolvedOutputDir)) {
    New-Item -ItemType Directory -Path $resolvedOutputDir -Force | Out-Null
}
Assert-NoReparsePointsBetween -RootPath $releaseRoot -TargetPath $resolvedOutputDir -Context "Tea release asset output"
if (((Test-Path -LiteralPath $zipPath -PathType Leaf) -or (Test-Path -LiteralPath $shaPath -PathType Leaf)) -and -not $Force) {
    throw "Release zip already exists. Re-run with -Force to replace it: $zipPath"
}
if (Test-Path -LiteralPath $zipPath -PathType Leaf) {
    Remove-Item -LiteralPath $zipPath -Force
}
if (Test-Path -LiteralPath $shaPath -PathType Leaf) {
    Remove-Item -LiteralPath $shaPath -Force
}

$sourceSidecarContent = [System.IO.File]::ReadAllText($sourceSha)
$sourceSidecarMatch = [System.Text.RegularExpressions.Regex]::Match(
    $sourceSidecarContent,
    '\A([0-9a-fA-F]{64})  ([^\r\n]+)\r\n\z',
    [System.Text.RegularExpressions.RegexOptions]::CultureInvariant
)
if (-not $sourceSidecarMatch.Success) {
    throw "Tea source ZIP sidecar must contain one canonical '<64-hex>  <zip-name>' CRLF-terminated record: $sourceSha"
}
$expectedSourceHash = $sourceSidecarMatch.Groups[1].Value.ToLowerInvariant()
$sourceSidecarName = $sourceSidecarMatch.Groups[2].Value
if ($sourceSidecarName -ne (Split-Path -Leaf $sourceZip)) {
    throw "Tea source ZIP sidecar file name does not match the ZIP artifact: $sourceSidecarName"
}

Copy-Item -LiteralPath $sourceZip -Destination $zipPath -Force

$hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($hash -ne $expectedSourceHash) {
    throw "Tea source ZIP changed after package verification. Expected=$expectedSourceHash Actual=$hash"
}
[System.IO.File]::WriteAllText($shaPath, "$hash  $assetName`r`n", [System.Text.ASCIIEncoding]::new())

Write-Host "[tea-release-package] Created:"
Write-Host "  $zipPath"
