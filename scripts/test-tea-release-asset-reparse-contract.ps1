[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDir
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$packagePath = (Resolve-Path -LiteralPath $PackageDir).Path
$releaseRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $packagePath))
$outsideParent = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".tmp\release-asset-reparse-contract"))
$artifactRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $outsideParent ([System.Guid]::NewGuid().ToString("N")))
)
$outsidePath = Join-Path $artifactRoot "outside"
$junctionName = ".tmp-asset-reparse-contract-$([System.Guid]::NewGuid().ToString('N'))"
$junctionPath = [System.IO.Path]::GetFullPath((Join-Path $releaseRoot $junctionName))
$releasePrefix = $releaseRoot.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
$artifactPrefix = $outsideParent.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
$packageScript = Join-Path $PSScriptRoot "package-release-zip.ps1"
$assetName = "tea-windows-x64-V0.0.0.zip"
$junctionCreated = $false

if (-not $junctionPath.StartsWith($releasePrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
    -not (Split-Path -Leaf $junctionPath).StartsWith(".tmp-asset-reparse-contract-", [System.StringComparison]::Ordinal)) {
    throw "Release asset reparse contract junction escaped the Tea release root: $junctionPath"
}
if (-not $artifactRoot.StartsWith($artifactPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Release asset reparse contract target escaped Tea .tmp: $artifactRoot"
}

try {
    [void][System.IO.Directory]::CreateDirectory($outsidePath)
    New-Item -ItemType Junction -Path $junctionPath -Target $outsidePath | Out-Null
    $junctionCreated = $true

    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = @(& powershell.exe `
            -NoProfile `
            -ExecutionPolicy Bypass `
            -File $packageScript `
            -PackageDir $packagePath `
            -Tag V0.0.0 `
            -OutputDir $junctionPath `
            -AllowDirtyManifest 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }

    $outputText = $output -join "`n"
    if ($exitCode -eq 0) {
        throw "Tea release asset packaging accepted a junction output directory"
    }
    if ($outputText -notmatch "reparse point") {
        throw "Tea release asset packaging rejected the junction for an unexpected reason: $outputText"
    }
    foreach ($outsideArtifact in @(
        (Join-Path $outsidePath $assetName),
        (Join-Path $outsidePath "$assetName.sha256")
    )) {
        if (Test-Path -LiteralPath $outsideArtifact) {
            throw "Tea release asset packaging wrote outside the release root before rejecting the junction: $outsideArtifact"
        }
    }

    [ordered]@{
        status = "passed"
        packageScript = $packageScript
        rejectedOutput = $junctionPath
        outsideTarget = $outsidePath
        verifierExitCode = $exitCode
    } | ConvertTo-Json -Depth 4
} finally {
    if ($junctionCreated -and (Test-Path -LiteralPath $junctionPath)) {
        [System.IO.Directory]::Delete($junctionPath)
    }
    if (Test-Path -LiteralPath $artifactRoot) {
        $resolvedArtifactRoot = [System.IO.Path]::GetFullPath($artifactRoot)
        if (-not $resolvedArtifactRoot.StartsWith($artifactPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to clean release asset reparse artifacts outside Tea .tmp: $resolvedArtifactRoot"
        }
        Remove-Item -LiteralPath $resolvedArtifactRoot -Recurse -Force
    }
}

# The packaging failure above is the expected negative probe. Once its reason
# and containment behavior have been asserted, this contract must report
# success to CI rather than leaking the child verifier's non-zero exit code.
exit 0
