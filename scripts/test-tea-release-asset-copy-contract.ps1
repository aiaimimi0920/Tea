[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$packagePath = (Resolve-Path -LiteralPath $PackageDir).Path
$releaseRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $packagePath))
$releasePrefix = $releaseRoot.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
$tempName = ".tmp-asset-copy-contract-$([System.Guid]::NewGuid().ToString('N'))"
$outputPath = [System.IO.Path]::GetFullPath((Join-Path $releaseRoot $tempName))
if (-not $outputPath.StartsWith($releasePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Release asset contract output escaped the package release root: $outputPath"
}

$assetName = "tea-windows-x64-V0.0.0.zip"
$zipPath = Join-Path $outputPath $assetName
$sidecarPath = "$zipPath.sha256"
$packageScript = Join-Path $PSScriptRoot "package-release-zip.ps1"

try {
    & powershell.exe `
        -NoProfile `
        -ExecutionPolicy Bypass `
        -File $packageScript `
        -PackageDir $packagePath `
        -Tag V0.0.0 `
        -OutputDir $outputPath `
        -AllowDirtyManifest
    if ($LASTEXITCODE -ne 0) {
        throw "Tea release asset packaging contract failed with exit code $LASTEXITCODE"
    }

    $actualHash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $sidecar = [System.IO.File]::ReadAllText($sidecarPath)
    $match = [System.Text.RegularExpressions.Regex]::Match(
        $sidecar,
        '\A([0-9a-fA-F]{64})  ([^\r\n]+)\r\n\z',
        [System.Text.RegularExpressions.RegexOptions]::CultureInvariant
    )
    if (-not $match.Success) {
        throw "Copied Tea release asset sidecar is not canonical: $sidecarPath"
    }
    $expectedHash = $match.Groups[1].Value.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "Copied Tea release asset hash mismatch. Expected=$expectedHash Actual=$actualHash"
    }
    if ($match.Groups[2].Value -ne $assetName) {
        throw "Copied Tea release asset sidecar names another file: $($match.Groups[2].Value)"
    }

    [ordered]@{
        status = "passed"
        sourcePackage = $packagePath
        copiedBytes = (Get-Item -LiteralPath $zipPath).Length
        sha256 = $actualHash
    } | ConvertTo-Json -Depth 4
} finally {
    $resolvedOutputPath = [System.IO.Path]::GetFullPath($outputPath)
    if (-not $resolvedOutputPath.StartsWith($releasePrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not (Split-Path -Leaf $resolvedOutputPath).StartsWith(".tmp-asset-copy-contract-", [System.StringComparison]::Ordinal)) {
        throw "Refusing to clean unexpected release asset contract path: $resolvedOutputPath"
    }
    foreach ($path in @($zipPath, $sidecarPath)) {
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            Remove-Item -LiteralPath $path -Force
        }
    }
    if (Test-Path -LiteralPath $resolvedOutputPath -PathType Container) {
        [System.IO.Directory]::Delete($resolvedOutputPath, $false)
    }
}
