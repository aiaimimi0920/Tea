[CmdletBinding()]
param(
    [string]$OutputDir = "release\Tea",
    [string]$VersionId = "",
    [switch]$Force,
    [switch]$NoZip,
    [switch]$DryRun,
    [switch]$AllowDirtyManifest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$teaRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$outputRoot = if ([System.IO.Path]::IsPathRooted($OutputDir)) {
    [System.IO.Path]::GetFullPath($OutputDir)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $teaRoot $OutputDir))
}

function Write-Utf8NoBom {
    param(
        [string]$Path,
        [string]$Value
    )

    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($Path, $Value, $encoding)
}

function Write-AsciiFile {
    param(
        [string]$Path,
        [string]$Value
    )

    $encoding = [System.Text.ASCIIEncoding]::new()
    [System.IO.File]::WriteAllText($Path, $Value, $encoding)
}

function Get-GitText {
    param([string[]]$Arguments)

    try {
        $output = & git -C $teaRoot @Arguments 2>$null
        if ($LASTEXITCODE -ne 0) { return "" }
        return (($output | Select-Object -First 1) -as [string]).Trim()
    } catch {
        return ""
    }
}

function Get-GitDirty {
    try {
        $output = & git -C $teaRoot status --porcelain 2>$null
        if ($LASTEXITCODE -ne 0) { return $null }
        return -not [string]::IsNullOrWhiteSpace(($output -join ""))
    } catch {
        return $null
    }
}

function Get-DefaultVersionId {
    $shortSha = Get-GitText -Arguments @("rev-parse", "--short=8", "HEAD")
    if ([string]::IsNullOrWhiteSpace($shortSha)) { $shortSha = "nogit" }
    return "dev-$(Get-Date -Format 'yyyyMMdd-HHmmss')-$shortSha"
}

function Get-RelativePathCompat {
    # Windows PowerShell 5.1 lacks [System.IO.Path]::GetRelativePath (a .NET Core 2.1+ API),
    # so compute the relative path via System.Uri, which works on both 5.1 and pwsh 7+.
    param(
        [string]$BasePath,
        [string]$Path
    )

    $baseFull = [System.IO.Path]::GetFullPath($BasePath)
    $sep = [System.IO.Path]::DirectorySeparatorChar
    if (-not $baseFull.EndsWith($sep)) { $baseFull += $sep }
    $targetFull = [System.IO.Path]::GetFullPath($Path)
    $baseUri = New-Object System.Uri($baseFull)
    $targetUri = New-Object System.Uri($targetFull)
    $relative = [System.Uri]::UnescapeDataString($baseUri.MakeRelativeUri($targetUri).ToString())
    return $relative
}

function New-FileRecord {
    param(
        [string]$BasePath,
        [string]$Path,
        [string]$Kind
    )

    $relative = (Get-RelativePathCompat -BasePath $BasePath -Path $Path).Replace("/", "\")
    return [ordered]@{
        kind = $Kind
        path = $relative
        sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
        bytes = (Get-Item -LiteralPath $Path).Length
    }
}

function Copy-ReleaseFile {
    param(
        [string]$Source,
        [string]$Destination,
        [string]$Kind,
        [string]$BasePath
    )

    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        throw "Missing release source file: $Source"
    }
    $destinationDir = Split-Path -Parent $Destination
    if (-not (Test-Path -LiteralPath $destinationDir)) {
        New-Item -ItemType Directory -Path $destinationDir -Force | Out-Null
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
    return New-FileRecord -BasePath $BasePath -Path $Destination -Kind $Kind
}

function Write-Checksums {
    param([string]$Destination)

    $checksumPath = Join-Path $Destination "checksums.sha256"
    $lines = New-Object System.Collections.Generic.List[string]
    Get-ChildItem -LiteralPath $Destination -Recurse -File |
        Where-Object { $_.FullName -ne $checksumPath } |
        Sort-Object FullName |
        ForEach-Object {
            $relative = (Get-RelativePathCompat -BasePath $Destination -Path $_.FullName).Replace("/", "\")
            $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            $lines.Add("$hash  $relative")
        }
    Write-AsciiFile -Path $checksumPath -Value (($lines -join "`r`n") + "`r`n")
}

function New-ZipPackage {
    param(
        [string]$Destination,
        [string]$VersionIdValue,
        [string[]]$PayloadRelativePaths
    )

    $packageDir = Join-Path $Destination "packages"
    New-Item -ItemType Directory -Path $packageDir -Force | Out-Null
    $zipPath = Join-Path $packageDir "Tea-$VersionIdValue-windows-x64.zip"
    $zipShaPath = "$zipPath.sha256"
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }
    if (Test-Path -LiteralPath $zipShaPath) { Remove-Item -LiteralPath $zipShaPath -Force }

    $stagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("tea-release-package-" + [System.Guid]::NewGuid().ToString("N"))
    try {
        New-Item -ItemType Directory -Path $stagingRoot -Force | Out-Null
        foreach ($relative in $PayloadRelativePaths) {
            $source = Join-Path $Destination $relative
            $target = Join-Path $stagingRoot $relative
            $targetDir = Split-Path -Parent $target
            if (-not (Test-Path -LiteralPath $targetDir)) { New-Item -ItemType Directory -Path $targetDir -Force | Out-Null }
            Copy-Item -LiteralPath $source -Destination $target -Force
        }
        Compress-Archive -Path (Join-Path $stagingRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal
    } finally {
        if (Test-Path -LiteralPath $stagingRoot) { Remove-Item -LiteralPath $stagingRoot -Recurse -Force }
    }

    $hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-AsciiFile -Path $zipShaPath -Value "$hash  $(Split-Path -Leaf $zipPath)`r`n"
    return @(
        (New-FileRecord -BasePath $Destination -Path $zipPath -Kind "zip"),
        (New-FileRecord -BasePath $Destination -Path $zipShaPath -Kind "zip-sha256")
    )
}

$versionIdValue = if ([string]::IsNullOrWhiteSpace($VersionId)) { Get-DefaultVersionId } else { $VersionId }
$destination = Join-Path $outputRoot $versionIdValue
$desktopRoot = Join-Path $teaRoot "apps\desktop"
$desktopExe = Join-Path $desktopRoot "src-tauri\target\release\tea.exe"
$workspaceTarget = Join-Path $teaRoot "target\release"
$exes = @(
    [ordered]@{ name = "tea.exe"; source = $desktopExe },
    [ordered]@{ name = "tea-daemon.exe"; source = (Join-Path $workspaceTarget "tea-daemon.exe") },
    [ordered]@{ name = "tea-cli.exe"; source = (Join-Path $workspaceTarget "tea-cli.exe") },
    [ordered]@{ name = "tea-mcp.exe"; source = (Join-Path $workspaceTarget "tea-mcp.exe") },
    [ordered]@{ name = "tea-sync.exe"; source = (Join-Path $workspaceTarget "tea-sync.exe") }
)
$supportFiles = @(
    [ordered]@{ path = "start-tea.bat"; source = (Join-Path $teaRoot "scripts\start-tea.bat") },
    [ordered]@{ path = "start-tea-daemon.bat"; source = (Join-Path $teaRoot "scripts\start-tea-daemon.bat") },
    [ordered]@{ path = "stop-tea.bat"; source = (Join-Path $teaRoot "scripts\stop-tea.bat") }
)
$commands = @(
    [ordered]@{ display = "cargo clean --manifest-path Cargo.toml"; executable = "cargo"; arguments = @("clean", "--manifest-path", "Cargo.toml"); workingDirectory = $teaRoot },
    [ordered]@{ display = "cargo build --manifest-path Cargo.toml --locked --release -p tea-daemon -p tea-cli -p tea-mcp -p tea-sync"; executable = "cargo"; arguments = @("build", "--manifest-path", "Cargo.toml", "--locked", "--release", "-p", "tea-daemon", "-p", "tea-cli", "-p", "tea-mcp", "-p", "tea-sync"); workingDirectory = $teaRoot },
    [ordered]@{ display = "npm run tauri build -- --no-bundle"; executable = "cmd.exe"; arguments = @("/d", "/c", "npm run tauri build -- --no-bundle"); workingDirectory = $desktopRoot }
)

if ($DryRun) {
    [ordered]@{
        schemaVersion = 1
        app = "Tea"
        teaRoot = $teaRoot
        outputRoot = $outputRoot
        destination = $destination
        versionId = $versionIdValue
        commands = $commands
        exes = $exes
        supportFiles = $supportFiles
        zip = (-not $NoZip)
    } | ConvertTo-Json -Depth 8
    exit 0
}

$gitDirty = Get-GitDirty
if (($gitDirty -eq $true) -and -not $AllowDirtyManifest) {
    throw "Refusing to create a formal Tea release package from a dirty working tree. Commit/stash changes or pass -AllowDirtyManifest for local smoke artifacts."
}

if ((Test-Path -LiteralPath $destination) -and -not $Force) {
    throw "Release destination already exists. Re-run with -Force to replace it: $destination"
}
if (Test-Path -LiteralPath $destination) {
    Remove-Item -LiteralPath $destination -Recurse -Force
}
New-Item -ItemType Directory -Path $destination -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $destination "logs") -Force | Out-Null
if (-not $NoZip) { New-Item -ItemType Directory -Path (Join-Path $destination "packages") -Force | Out-Null }

foreach ($command in $commands) {
    Write-Host ">> $($command.display)"
    Push-Location -LiteralPath ([string]$command.workingDirectory)
    try {
        & ([string]$command.executable) @($command.arguments)
        if ($LASTEXITCODE -ne 0) { throw "Command failed with exit code $LASTEXITCODE`: $($command.display)" }
    } finally {
        Pop-Location
    }
}

$exeRecords = @()
foreach ($exe in $exes) {
    $exeRecords += Copy-ReleaseFile -Source ([string]$exe.source) -Destination (Join-Path $destination ([string]$exe.name)) -Kind "exe" -BasePath $destination
}
$supportRecords = @()
foreach ($support in $supportFiles) {
    $supportRecords += Copy-ReleaseFile -Source ([string]$support.source) -Destination (Join-Path $destination ([string]$support.path)) -Kind "support" -BasePath $destination
}

$payloadRelativePaths = @($exeRecords + $supportRecords | ForEach-Object { [string]$_.path })
$artifactRecords = @()
if (-not $NoZip) {
    $artifactRecords += New-ZipPackage -Destination $destination -VersionIdValue $versionIdValue -PayloadRelativePaths $payloadRelativePaths
}

$gitHead = Get-GitText -Arguments @("rev-parse", "HEAD")
$gitShortSha = Get-GitText -Arguments @("rev-parse", "--short=8", "HEAD")
if ([string]::IsNullOrWhiteSpace($gitHead)) { $gitHead = "nogit" }
if ([string]::IsNullOrWhiteSpace($gitShortSha)) { $gitShortSha = "nogit" }
$builtAt = Get-Date -Format o
$buildInfoPath = Join-Path $destination "BUILD_INFO.txt"
$buildInfo = @(
    "Tea Windows release artifact"
    "Built at: $builtAt"
    "Version ID: $versionIdValue"
    "Git HEAD: $gitHead"
    "Git dirty: $gitDirty"
    "Tea root: $teaRoot"
) -join [Environment]::NewLine
Write-Utf8NoBom -Path $buildInfoPath -Value ($buildInfo + [Environment]::NewLine)
$buildInfoRecord = New-FileRecord -BasePath $destination -Path $buildInfoPath -Kind "build-info"

$manifest = [ordered]@{
    schemaVersion = 1
    app = "Tea"
    sourceProject = "Tea"
    versionId = $versionIdValue
    builtAt = $builtAt
    gitHead = $gitHead
    gitShortSha = $gitShortSha
    gitDirty = $gitDirty
    profile = "release"
    target = "windows-x64"
    teaRoot = $teaRoot
    destination = $destination
    exes = $exeRecords
    supportFiles = $supportRecords
    buildInfo = $buildInfoRecord
    buildLogs = @()
    artifacts = $artifactRecords
    checksums = "checksums.sha256"
}
$manifestPath = Join-Path $destination "manifest.json"
Write-Utf8NoBom -Path $manifestPath -Value (($manifest | ConvertTo-Json -Depth 12) + [Environment]::NewLine)
Write-Checksums -Destination $destination

Write-Host "[tea-local-build] Release artifacts ready: $destination"
[ordered]@{
    status = "passed"
    app = "Tea"
    destination = $destination
    versionId = $versionIdValue
    gitHead = $gitHead
    gitDirty = $gitDirty
    exes = @($exeRecords | ForEach-Object { $_.path })
    artifacts = @($artifactRecords | ForEach-Object { $_.path })
    manifest = "manifest.json"
    checksums = "checksums.sha256"
} | ConvertTo-Json -Depth 8