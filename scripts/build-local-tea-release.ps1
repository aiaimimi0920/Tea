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
$deterministicZipTimestamp = [System.DateTimeOffset]::new(
    1980,
    1,
    1,
    0,
    0,
    0,
    [System.TimeSpan]::Zero
)

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

function Assert-NotReparsePoint {
    param(
        [System.IO.FileSystemInfo]$Item,
        [string]$Context
    )

    if (($Item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "$Context must not be a symbolic link, junction, or other reparse point: $($Item.FullName)"
    }
}

function Get-FilesWithoutReparsePoints {
    param(
        [string]$RootPath,
        [string]$Context
    )

    $root = Get-Item -Force -LiteralPath $RootPath
    if (-not $root.PSIsContainer) {
        throw "$Context root must be a directory: $RootPath"
    }
    Assert-NotReparsePoint -Item $root -Context "$Context root"

    $directories = [System.Collections.Generic.Stack[System.IO.DirectoryInfo]]::new()
    $directories.Push([System.IO.DirectoryInfo]$root)
    $files = [System.Collections.Generic.List[System.IO.FileInfo]]::new()
    while ($directories.Count -gt 0) {
        $directory = $directories.Pop()
        foreach ($entry in $directory.EnumerateFileSystemInfos()) {
            Assert-NotReparsePoint -Item $entry -Context "$Context entry"
            if (($entry.Attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
                $directories.Push([System.IO.DirectoryInfo]$entry)
            } else {
                $files.Add([System.IO.FileInfo]$entry)
            }
        }
    }
    return $files.ToArray()
}

function Assert-PathContainsNoReparsePoints {
    param(
        [string]$RootPath,
        [string]$Path,
        [string]$Context
    )

    $rootFull = [System.IO.Path]::GetFullPath($RootPath)
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    Assert-NotReparsePoint -Item (Get-Item -Force -LiteralPath $rootFull) -Context "$Context root"
    $prefix = $rootFull.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
    if (-not $pathFull.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Context path escapes its root: $pathFull"
    }
    $current = $rootFull
    foreach ($segment in $pathFull.Substring($prefix.Length).Split(
        [char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar),
        [System.StringSplitOptions]::RemoveEmptyEntries
    )) {
        $current = Join-Path $current $segment
        if (Test-Path -LiteralPath $current) {
            Assert-NotReparsePoint -Item (Get-Item -Force -LiteralPath $current) -Context $Context
        }
    }
}

function Get-CommonAncestorPath {
    param(
        [string]$FirstPath,
        [string]$SecondPath
    )

    $candidate = [System.IO.Path]::GetFullPath($FirstPath)
    $secondFull = [System.IO.Path]::GetFullPath($SecondPath)
    while (-not [string]::IsNullOrWhiteSpace($candidate)) {
        $candidatePrefix = $candidate.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
        if (
            $secondFull.Equals($candidate, [System.StringComparison]::OrdinalIgnoreCase) -or
            $secondFull.StartsWith($candidatePrefix, [System.StringComparison]::OrdinalIgnoreCase)
        ) {
            return $candidate
        }

        $parent = [System.IO.Directory]::GetParent($candidate)
        if ($null -eq $parent) { break }
        $candidate = $parent.FullName
    }

    throw "Tea release source and OutputDir do not share a filesystem root: $FirstPath, $SecondPath"
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
        name = Split-Path -Leaf $relative
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
    Assert-NotReparsePoint -Item (Get-Item -Force -LiteralPath $Source) -Context "Release source file"
    $destinationDir = Split-Path -Parent $Destination
    if (-not (Test-Path -LiteralPath $destinationDir)) {
        New-Item -ItemType Directory -Path $destinationDir -Force | Out-Null
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
    return New-FileRecord -BasePath $BasePath -Path $Destination -Kind $Kind
}

function Invoke-ReleaseBuildCommands {
    param([object[]]$Commands)

    foreach ($command in $Commands) {
        Write-Host ">> $($command.display)"
        Push-Location -LiteralPath ([string]$command.workingDirectory)
        try {
            & ([string]$command.executable) @($command.arguments)
            if ($LASTEXITCODE -ne 0) {
                throw "Command failed with exit code $LASTEXITCODE`: $($command.display)"
            }
        } finally {
            Pop-Location
        }
    }
}

function Write-Checksums {
    param([string]$Destination)

    $checksumPath = Join-Path $Destination "checksums.sha256"
    $lines = New-Object System.Collections.Generic.List[string]
    Get-FilesWithoutReparsePoints -RootPath $Destination -Context "Release package" |
        Where-Object { $_.FullName -ne $checksumPath } |
        Sort-Object FullName |
        ForEach-Object {
            $relative = (Get-RelativePathCompat -BasePath $Destination -Path $_.FullName).Replace("/", "\")
            $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            $lines.Add("$hash  $relative")
        }
    Write-AsciiFile -Path $checksumPath -Value (($lines -join "`r`n") + "`r`n")
}

function Sort-OrdinalStrings {
    param([string[]]$Values)

    $sorted = [string[]]@($Values)
    [System.Array]::Sort($sorted, [System.StringComparer]::Ordinal)
    return $sorted
}

function Write-DeterministicZip {
    param(
        [string]$SourceRoot,
        [string]$ZipPath,
        [string[]]$PayloadRelativePaths
    )

    Add-Type -AssemblyName System.IO.Compression
    $sourceRootFull = [System.IO.Path]::GetFullPath($SourceRoot)
    $sourcePrefix = $sourceRootFull.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
    $zipPathFull = [System.IO.Path]::GetFullPath($ZipPath)
    if (Test-Path -LiteralPath $zipPathFull) {
        throw "Deterministic ZIP destination already exists: $zipPathFull"
    }

    $entryNames = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    $entries = New-Object System.Collections.Generic.List[object]
    foreach ($relative in (Sort-OrdinalStrings -Values $PayloadRelativePaths)) {
        if ([string]::IsNullOrWhiteSpace($relative) -or [System.IO.Path]::IsPathRooted($relative)) {
            throw "Tea ZIP payload path must be a non-empty relative path: $relative"
        }
        $sourcePath = [System.IO.Path]::GetFullPath((Join-Path $sourceRootFull $relative))
        if (-not $sourcePath.StartsWith($sourcePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Tea ZIP payload path escapes the package directory: $relative"
        }
        if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
            throw "Tea ZIP payload file is missing: $sourcePath"
        }
        Assert-PathContainsNoReparsePoints -RootPath $sourceRootFull -Path $sourcePath -Context "Tea ZIP payload"
        $entryName = $relative.Replace("\", "/")
        if ($entryName.StartsWith("/") -or $entryName -match '(^|/)\.\.?(?:/|$)') {
            throw "Tea ZIP payload path is unsafe: $relative"
        }
        if (-not $entryNames.Add($entryName)) {
            throw "Tea ZIP payload path is duplicated: $entryName"
        }
        $entries.Add([pscustomobject]@{
            sourcePath = $sourcePath
            entryName = $entryName
        })
    }

    try {
        $zipStream = [System.IO.File]::Open(
            $zipPathFull,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        try {
            $archive = [System.IO.Compression.ZipArchive]::new(
                $zipStream,
                [System.IO.Compression.ZipArchiveMode]::Create,
                $true
            )
            try {
                foreach ($entrySource in $entries) {
                    $entry = $archive.CreateEntry(
                        [string]$entrySource.entryName,
                        [System.IO.Compression.CompressionLevel]::Optimal
                    )
                    $entry.LastWriteTime = $deterministicZipTimestamp
                    $entry.ExternalAttributes = 0
                    $input = [System.IO.File]::OpenRead([string]$entrySource.sourcePath)
                    try {
                        $output = $entry.Open()
                        try {
                            $input.CopyTo($output)
                        } finally {
                            $output.Dispose()
                        }
                    } finally {
                        $input.Dispose()
                    }
                }
            } finally {
                $archive.Dispose()
            }
        } finally {
            $zipStream.Dispose()
        }
    } catch {
        Remove-Item -LiteralPath $zipPathFull -Force -ErrorAction SilentlyContinue
        throw
    }
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

    Write-DeterministicZip -SourceRoot $Destination -ZipPath $zipPath -PayloadRelativePaths $PayloadRelativePaths

    $verificationZip = Join-Path ([System.IO.Path]::GetTempPath()) ("tea-release-repro-" + [System.Guid]::NewGuid().ToString("N") + ".zip")
    try {
        Write-DeterministicZip -SourceRoot $Destination -ZipPath $verificationZip -PayloadRelativePaths $PayloadRelativePaths
        $firstHash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $verificationHash = (Get-FileHash -LiteralPath $verificationZip -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($firstHash -ne $verificationHash) {
            throw "Tea ZIP reproducibility check failed for identical payloads. First=$firstHash Verification=$verificationHash"
        }
    } finally {
        Remove-Item -LiteralPath $verificationZip -Force -ErrorAction SilentlyContinue
    }

    $hash = $firstHash
    Write-AsciiFile -Path $zipShaPath -Value "$hash  $(Split-Path -Leaf $zipPath)`r`n"
    return @(
        (New-FileRecord -BasePath $Destination -Path $zipPath -Kind "zip"),
        (New-FileRecord -BasePath $Destination -Path $zipShaPath -Kind "zip-sha256")
    )
}

$versionIdValue = if ([string]::IsNullOrWhiteSpace($VersionId)) { Get-DefaultVersionId } else { $VersionId }
if ($versionIdValue -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$') {
    throw "Tea VersionId must be one safe path segment containing only letters, digits, dot, underscore, or hyphen: $versionIdValue"
}
$destination = [System.IO.Path]::GetFullPath((Join-Path $outputRoot $versionIdValue))
$outputPrefix = $outputRoot.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
if (-not $destination.StartsWith($outputPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Tea release destination escapes OutputDir: $destination"
}
$trustedOutputAncestor = Get-CommonAncestorPath -FirstPath $teaRoot -SecondPath $outputRoot
Assert-PathContainsNoReparsePoints `
    -RootPath $trustedOutputAncestor `
    -Path $outputRoot `
    -Context "Tea release OutputDir"
Assert-PathContainsNoReparsePoints `
    -RootPath $trustedOutputAncestor `
    -Path $destination `
    -Context "Tea release destination"
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
    [ordered]@{ path = "stop-tea.bat"; source = (Join-Path $teaRoot "scripts\stop-tea.bat") },
    [ordered]@{ path = "resolve-tea-token.ps1"; source = (Join-Path $teaRoot "scripts\resolve-tea-token.ps1") },
    [ordered]@{ path = "start-tea-daemon.ps1"; source = (Join-Path $teaRoot "scripts\start-tea-daemon.ps1") },
    [ordered]@{ path = "stop-tea.ps1"; source = (Join-Path $teaRoot "scripts\stop-tea.ps1") }
)
$commands = @(
    [ordered]@{ display = "cargo build --manifest-path Cargo.toml --locked --release -p tea-daemon -p tea-cli -p tea-mcp -p tea-sync"; executable = "cargo"; arguments = @("build", "--manifest-path", "Cargo.toml", "--locked", "--release", "-p", "tea-daemon", "-p", "tea-cli", "-p", "tea-mcp", "-p", "tea-sync"); workingDirectory = $teaRoot },
    [ordered]@{ display = "npm run tauri build -- --no-bundle"; executable = "cmd.exe"; arguments = @("/d", "/c", "npm run tauri build -- --no-bundle"); workingDirectory = $desktopRoot }
)

if ($DryRun) {
    [ordered]@{
        schemaVersion = 3
        app = "Tea"
        teaRoot = $teaRoot
        outputRoot = $outputRoot
        destination = $destination
        versionId = $versionIdValue
        commands = $commands
        exes = $exes
        supportFiles = $supportFiles
        zip = (-not $NoZip)
        deterministicZip = (-not $NoZip)
        binaryRepeatBuildVerification = $true
    } | ConvertTo-Json -Depth 8
    exit 0
}

$gitDirty = Get-GitDirty
if ($null -eq $gitDirty) {
    throw "Unable to determine Tea Git working-tree state; refusing to create a release manifest."
}
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

try {
Invoke-ReleaseBuildCommands -Commands $commands
$firstBuildSnapshots = @{}
foreach ($exe in $exes) {
    $sourcePath = [string]$exe.source
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
        throw "Missing first-build executable for repeatability verification: $sourcePath"
    }
    $firstBuildSnapshots[[string]$exe.name] = [ordered]@{
        sha256 = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
        bytes = (Get-Item -LiteralPath $sourcePath).Length
    }
}

Write-Host ">> repeating release build commands for binary reproducibility verification"
Invoke-ReleaseBuildCommands -Commands $commands
$binaryReproducibilityRecords = @()
foreach ($exe in $exes) {
    $name = [string]$exe.name
    $sourcePath = [string]$exe.source
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
        throw "Missing second-build executable for repeatability verification: $sourcePath"
    }
    $first = $firstBuildSnapshots[$name]
    $secondHash = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $secondBytes = (Get-Item -LiteralPath $sourcePath).Length
    if ([string]$first.sha256 -ne $secondHash -or [long]$first.bytes -ne [long]$secondBytes) {
        throw "Tea binary repeat-build verification failed for $name. First=$($first.sha256)/$($first.bytes) Second=$secondHash/$secondBytes"
    }
    $binaryReproducibilityRecords += [ordered]@{
        name = $name
        firstSha256 = [string]$first.sha256
        secondSha256 = $secondHash
        bytes = [long]$secondBytes
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
    "Binary repeat build: verified (same-worktree-repeat-build, clean builds: false)"
) -join [Environment]::NewLine
Write-Utf8NoBom -Path $buildInfoPath -Value ($buildInfo + [Environment]::NewLine)
$buildInfoRecord = New-FileRecord -BasePath $destination -Path $buildInfoPath -Kind "build-info"

$zipPackaging = if (-not $NoZip) {
    [ordered]@{
        deterministic = $true
        entryOrder = "ordinal"
        entryTimestamp = $deterministicZipTimestamp.ToString(
            "yyyy-MM-ddTHH:mm:ss",
            [System.Globalization.CultureInfo]::InvariantCulture
        )
        compressionLevel = "optimal"
        verification = "same-payload-double-build"
    }
} else {
    $null
}
$manifest = [ordered]@{
    schemaVersion = 3
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
    packaging = [ordered]@{
        zip = $zipPackaging
    }
    reproducibility = [ordered]@{
        schemaVersion = 1
        binaries = [ordered]@{
            status = "verified"
            method = "same-worktree-repeat-build"
            cleanBuilds = $false
            artifacts = $binaryReproducibilityRecords
        }
        zip = if (-not $NoZip) {
            [ordered]@{
                status = "verified"
                method = "same-payload-double-build"
                sha256 = [string](@($artifactRecords | Where-Object { $_.kind -eq "zip" })[0].sha256)
            }
        } else {
            [ordered]@{
                status = "not-produced"
                method = "none"
                sha256 = $null
            }
        }
    }
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
    deterministicZip = (-not $NoZip)
    binaryRepeatBuildVerified = $true
    manifest = "manifest.json"
    checksums = "checksums.sha256"
} | ConvertTo-Json -Depth 8
} catch {
    if (Test-Path -LiteralPath $destination) {
        Remove-Item -LiteralPath $destination -Recurse -Force -ErrorAction SilentlyContinue
    }
    throw
}
