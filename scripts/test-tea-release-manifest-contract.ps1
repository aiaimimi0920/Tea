[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$verifierPath = Join-Path $PSScriptRoot "verify-tea-release-package.ps1"
$artifactParent = [System.IO.Path]::GetFullPath(
    (Join-Path $repoRoot ".tmp\release-manifest-contract")
)
$artifactRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $artifactParent ([System.Guid]::NewGuid().ToString("N")))
)
$artifactPrefix = $artifactParent + [System.IO.Path]::DirectorySeparatorChar
if (-not $artifactRoot.StartsWith($artifactPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Release manifest contract artifact root escaped Tea .tmp: $artifactRoot"
}

$packagePath = Join-Path $artifactRoot "manifest-path-mismatch"
$requiredPayload = @(
    "tea.exe",
    "tea-daemon.exe",
    "tea-cli.exe",
    "tea-mcp.exe",
    "tea-sync.exe",
    "start-tea.bat",
    "start-tea-daemon.bat",
    "stop-tea.bat",
    "resolve-tea-token.ps1",
    "start-tea-daemon.ps1",
    "stop-tea.ps1",
    "checksums.sha256"
)
$exeNames = @("tea.exe", "tea-daemon.exe", "tea-cli.exe", "tea-mcp.exe", "tea-sync.exe")
$supportNames = @(
    "start-tea.bat",
    "start-tea-daemon.bat",
    "stop-tea.bat",
    "resolve-tea-token.ps1",
    "start-tea-daemon.ps1",
    "stop-tea.ps1"
)

try {
    [void][System.IO.Directory]::CreateDirectory($packagePath)
    foreach ($name in $requiredPayload) {
        [System.IO.File]::WriteAllText(
            (Join-Path $packagePath $name),
            "",
            [System.Text.UTF8Encoding]::new($false)
        )
    }

    $exeRecords = @()
    foreach ($name in $exeNames) {
        $path = if ($name -eq "tea.exe") { "alternate-tea.exe" } else { $name }
        $exeRecords += [ordered]@{
            kind = "exe"
            name = $name
            path = $path
            sha256 = "0" * 64
            bytes = 0
        }
    }
    $supportRecords = @()
    foreach ($name in $supportNames) {
        $supportRecords += [ordered]@{
            kind = "support"
            name = $name
            path = $name
            sha256 = "0" * 64
            bytes = 0
        }
    }
    $manifest = [ordered]@{
        schemaVersion = 1
        app = "Tea"
        sourceProject = "Tea"
        versionId = Split-Path -Leaf $packagePath
        gitHead = "0" * 40
        gitShortSha = "0" * 8
        gitDirty = $true
        profile = "release"
        target = "windows-x64"
        exes = $exeRecords
        supportFiles = $supportRecords
        buildInfo = $null
        buildLogs = @()
        artifacts = @()
        checksums = "checksums.sha256"
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $packagePath "manifest.json"),
        ($manifest | ConvertTo-Json -Depth 8),
        [System.Text.UTF8Encoding]::new($false)
    )

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
        throw "Tea release verifier accepted an executable manifest path mismatch"
    }
    if ($outputText -notmatch "executable\s+path\s+must\s+match\s+its\s+canonical\s+name") {
        throw "Tea release verifier rejected the malformed manifest for an unexpected reason: $outputText"
    }

    $emptyFileSha256 = (Get-FileHash -LiteralPath (Join-Path $packagePath "tea.exe") -Algorithm SHA256).Hash.ToLowerInvariant()
    foreach ($record in $exeRecords) {
        $record["path"] = $record["name"]
        $record["sha256"] = $emptyFileSha256
    }
    foreach ($record in $supportRecords) {
        $record["sha256"] = $emptyFileSha256
    }
    $outsideBuildInfoPath = Join-Path $artifactRoot "outside-build-info.txt"
    [System.IO.File]::WriteAllText(
        $outsideBuildInfoPath,
        "outside package",
        [System.Text.UTF8Encoding]::new($false)
    )
    $manifest["buildInfo"] = [ordered]@{
        kind = "build-info"
        name = "outside-build-info.txt"
        path = "..\outside-build-info.txt"
        sha256 = (Get-FileHash -LiteralPath $outsideBuildInfoPath -Algorithm SHA256).Hash.ToLowerInvariant()
        bytes = (Get-Item -LiteralPath $outsideBuildInfoPath).Length
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $packagePath "manifest.json"),
        ($manifest | ConvertTo-Json -Depth 8),
        [System.Text.UTF8Encoding]::new($false)
    )

    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $escapeOutput = @(& powershell.exe `
            -NoProfile `
            -ExecutionPolicy Bypass `
            -File $verifierPath `
            -PackageDir $packagePath `
            -AllowDirtyManifest 2>&1)
        $escapeExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    $escapeOutputText = $escapeOutput -join "`n"
    if ($escapeExitCode -eq 0) {
        throw "Tea release verifier accepted a build-info hash record outside the package"
    }
    if ($escapeOutputText -notmatch "manifest\s+file\s+path\s+escapes\s+package\s+directory") {
        throw "Tea release verifier rejected the escaping hash record for an unexpected reason: $escapeOutputText"
    }

    [ordered]@{
        status = "passed"
        verifier = $verifierPath
        rejectedExecutable = "tea.exe"
        rejectedPath = "alternate-tea.exe"
        verifierExitCode = $exitCode
        rejectedHashRecord = "buildInfo"
        rejectedHashPath = "..\outside-build-info.txt"
        hashEscapeVerifierExitCode = $escapeExitCode
    } | ConvertTo-Json -Depth 4
} finally {
    if (Test-Path -LiteralPath $artifactRoot) {
        $resolvedArtifactRoot = [System.IO.Path]::GetFullPath($artifactRoot)
        if (-not $resolvedArtifactRoot.StartsWith($artifactPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to clean release manifest artifacts outside Tea .tmp: $resolvedArtifactRoot"
        }
        Remove-Item -LiteralPath $resolvedArtifactRoot -Recurse -Force
    }
}

# Both verifier failures above are expected negative probes. Once their reasons
# have been asserted, this contract itself must report success to CI.
exit 0
