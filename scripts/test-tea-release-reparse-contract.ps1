[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$verifierPath = Join-Path $PSScriptRoot "verify-tea-release-package.ps1"
$buildScriptPath = Join-Path $PSScriptRoot "build-local-tea-release.ps1"
$artifactParent = [System.IO.Path]::GetFullPath(
    (Join-Path $repoRoot ".tmp\release-reparse-contract")
)
$artifactRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $artifactParent ([System.Guid]::NewGuid().ToString("N")))
)
if (-not $artifactRoot.StartsWith(
    $artifactParent + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw "Release reparse contract artifact root escaped Tea .tmp: $artifactRoot"
}

$packagePath = Join-Path $artifactRoot "package"
$outsidePath = Join-Path $artifactRoot "outside"
$junctionPath = Join-Path $packagePath "escape"
$buildOutputPath = Join-Path $artifactRoot "build-output"
$buildOutsidePath = Join-Path $artifactRoot "build-outside"
$buildVersionId = "reparse-version"
$buildDestinationPath = Join-Path $buildOutputPath $buildVersionId
$neuroRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot ".."))
$requiredReleaseRoot = [System.IO.Path]::GetFullPath((Join-Path $neuroRoot "release\Tea"))
$requiredReleaseVersionId = "reparse-ancestor-probe"
$junctionCreated = $false
$buildJunctionCreated = $false

try {
    [void][System.IO.Directory]::CreateDirectory($packagePath)
    [void][System.IO.Directory]::CreateDirectory($outsidePath)
    [System.IO.File]::WriteAllText(
        (Join-Path $outsidePath "outside.txt"),
        "must not be visible through the package boundary",
        [System.Text.UTF8Encoding]::new($false)
    )
    New-Item -ItemType Junction -Path $junctionPath -Target $outsidePath | Out-Null
    $junctionCreated = $true

    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = @(& powershell.exe `
            -NoProfile `
            -ExecutionPolicy Bypass `
            -File $verifierPath `
            -PackageDir $packagePath `
            -AllowDirtyManifest 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    $outputText = $output -join "`n"
    if ($exitCode -eq 0) {
        throw "Tea release verifier accepted a package containing a junction"
    }
    if ($outputText -notmatch "reparse point") {
        throw "Tea release verifier rejected the junction for an unexpected reason: $outputText"
    }

    [void][System.IO.Directory]::CreateDirectory($buildOutputPath)
    [void][System.IO.Directory]::CreateDirectory($buildOutsidePath)
    New-Item -ItemType Junction -Path $buildDestinationPath -Target $buildOutsidePath | Out-Null
    $buildJunctionCreated = $true
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $buildOutput = @(& powershell.exe `
            -NoProfile `
            -ExecutionPolicy Bypass `
            -File $buildScriptPath `
            -OutputDir $buildOutputPath `
            -VersionId $buildVersionId `
            -Force `
            -DryRun 2>&1)
        $buildExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    $buildOutputText = $buildOutput -join "`n"
    if ($buildExitCode -eq 0) {
        throw "Tea release builder accepted a reparse-point destination"
    }
    if ($buildOutputText -notmatch "reparse point") {
        throw "Tea release builder rejected the destination for an unexpected reason: $buildOutputText"
    }

    # The repository and its required sibling release root may both live below a
    # trusted NAS mount. Reject reparse points introduced after their shared
    # workspace ancestor, but do not reject that already-established mount.
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $requiredReleaseOutput = @(& powershell.exe `
            -NoProfile `
            -ExecutionPolicy Bypass `
            -File $buildScriptPath `
            -OutputDir $requiredReleaseRoot `
            -VersionId $requiredReleaseVersionId `
            -Force `
            -DryRun 2>&1)
        $requiredReleaseExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($requiredReleaseExitCode -ne 0) {
        throw "Tea release builder rejected the required sibling release root: $($requiredReleaseOutput -join "`n")"
    }
    $requiredReleasePlan = ($requiredReleaseOutput -join "`n") | ConvertFrom-Json
    $expectedRequiredDestination = Join-Path $requiredReleaseRoot $requiredReleaseVersionId
    if (-not ([string]$requiredReleasePlan.destination).Equals(
        $expectedRequiredDestination,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Tea release builder returned an unexpected required-root destination: $($requiredReleasePlan.destination)"
    }

    [ordered]@{
        status = "passed"
        verifier = $verifierPath
        rejectedEntry = $junctionPath
        outsideTarget = $outsidePath
        verifierExitCode = $exitCode
        builder = $buildScriptPath
        rejectedBuildDestination = $buildDestinationPath
        buildOutsideTarget = $buildOutsidePath
        builderExitCode = $buildExitCode
        acceptedRequiredReleaseRoot = $requiredReleaseRoot
        requiredReleaseExitCode = $requiredReleaseExitCode
    } | ConvertTo-Json -Depth 4
} finally {
    if ($buildJunctionCreated -and (Test-Path -LiteralPath $buildDestinationPath)) {
        [System.IO.Directory]::Delete($buildDestinationPath)
    }
    if ($junctionCreated -and (Test-Path -LiteralPath $junctionPath)) {
        [System.IO.Directory]::Delete($junctionPath)
    }
    if (Test-Path -LiteralPath $artifactRoot) {
        $resolvedArtifactRoot = [System.IO.Path]::GetFullPath($artifactRoot)
        if (-not $resolvedArtifactRoot.StartsWith(
            $artifactParent + [System.IO.Path]::DirectorySeparatorChar,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Refusing to clean release reparse artifacts outside Tea .tmp: $resolvedArtifactRoot"
        }
        Remove-Item -LiteralPath $resolvedArtifactRoot -Recurse -Force
    }
}

# The verifier's non-zero status is the expected negative probe result, not this
# contract's status. Do not leak it to the parent CI process after all assertions pass.
exit 0
