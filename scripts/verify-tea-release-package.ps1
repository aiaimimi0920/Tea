[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDir,
    [switch]$RunSmoke,
    [switch]$AllowDirtyManifest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$packagePath = (Resolve-Path -LiteralPath $PackageDir).Path

function Assert-True {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Equal {
    param(
        [object]$Expected,
        [object]$Actual,
        [string]$Message
    )

    if ($Expected -ne $Actual) {
        throw "$Message Expected=[$Expected] Actual=[$Actual]"
    }
}

function Get-RelativePath {
    param(
        [string]$BasePath,
        [string]$Path
    )

    $baseFull = [System.IO.Path]::GetFullPath($BasePath).TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    $baseUri = [System.Uri]::new($baseFull)
    $pathUri = [System.Uri]::new($pathFull)
    return [System.Uri]::UnescapeDataString($baseUri.MakeRelativeUri($pathUri).ToString()).Replace("/", "\")
}

function Test-HashRecord {
    param(
        [string]$RelativePath,
        [string]$ExpectedSha256
    )

    Assert-Sha256Value -Value $ExpectedSha256 -Context "SHA256 for $RelativePath"
    $path = Resolve-PackageFilePath -RelativePath $RelativePath
    Assert-True (Test-Path -LiteralPath $path -PathType Leaf) "Package file missing: $RelativePath"
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Equal $ExpectedSha256.ToLowerInvariant() $actual "SHA256 mismatch for $RelativePath."
}

function Assert-NotReparsePoint {
    param(
        [System.IO.FileSystemInfo]$Item,
        [string]$Context
    )

    $isReparsePoint = ($Item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
    Assert-True (-not $isReparsePoint) "$Context must not be a symbolic link, junction, or other reparse point: $($Item.FullName)"
}

function Get-PackageFilesWithoutReparsePoints {
    param([string]$RootPath)

    $root = Get-Item -Force -LiteralPath $RootPath
    Assert-True $root.PSIsContainer "Package root must be a directory: $RootPath"
    Assert-NotReparsePoint -Item $root -Context "Package root"

    $directories = [System.Collections.Generic.Stack[System.IO.DirectoryInfo]]::new()
    $directories.Push([System.IO.DirectoryInfo]$root)
    $files = [System.Collections.Generic.List[System.IO.FileInfo]]::new()
    while ($directories.Count -gt 0) {
        $directory = $directories.Pop()
        foreach ($entry in $directory.EnumerateFileSystemInfos()) {
            Assert-NotReparsePoint -Item $entry -Context "Package entry"
            if (($entry.Attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
                $directories.Push([System.IO.DirectoryInfo]$entry)
            } else {
                $files.Add([System.IO.FileInfo]$entry)
            }
        }
    }
    return $files.ToArray()
}

function Assert-PackagePathContainsNoReparsePoints {
    param([string]$Path)

    $root = Get-Item -Force -LiteralPath $packagePath
    Assert-NotReparsePoint -Item $root -Context "Package root"
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    $packagePrefix = $packagePath.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
    $relative = $pathFull.Substring($packagePrefix.Length)
    $current = $packagePath
    foreach ($segment in $relative.Split(
        [char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar),
        [System.StringSplitOptions]::RemoveEmptyEntries
    )) {
        $current = Join-Path $current $segment
        if (Test-Path -LiteralPath $current) {
            Assert-NotReparsePoint -Item (Get-Item -Force -LiteralPath $current) -Context "Package path component"
        }
    }
}

function Resolve-PackageFilePath {
    param([string]$RelativePath)

    Assert-True (-not [string]::IsNullOrWhiteSpace($RelativePath)) "Manifest file path must not be empty."
    Assert-True (-not [System.IO.Path]::IsPathRooted($RelativePath)) "Manifest file path must be relative: $RelativePath"
    $resolved = [System.IO.Path]::GetFullPath((Join-Path $packagePath $RelativePath))
    $packagePrefix = $packagePath.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
    Assert-True ($resolved.StartsWith($packagePrefix, [System.StringComparison]::OrdinalIgnoreCase)) "Manifest file path escapes package directory: $RelativePath"
    Assert-PackagePathContainsNoReparsePoints -Path $resolved
    return $resolved
}

function Sort-OrdinalStrings {
    param([string[]]$Values)

    $sorted = [string[]]@($Values)
    [System.Array]::Sort($sorted, [System.StringComparer]::Ordinal)
    return $sorted
}

function Get-ZipFileEntries {
    param([string]$ZipPath)

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
    try {
        return @($archive.Entries |
            Where-Object { -not [string]::IsNullOrEmpty($_.Name) } |
            ForEach-Object {
                [pscustomobject]@{
                    path = $_.FullName.Replace("\", "/")
                    timestamp = $_.LastWriteTime
                    externalAttributes = $_.ExternalAttributes
                }
            })
    } finally {
        $archive.Dispose()
    }
}

function Assert-Sha256Value {
    param(
        [string]$Value,
        [string]$Context
    )

    Assert-True ($Value -match '\A[0-9a-fA-F]{64}\z') "$Context must be exactly 64 hexadecimal characters."
}

$manifestPath = Join-Path $packagePath "manifest.json"
$checksumsPath = Join-Path $packagePath "checksums.sha256"
$teaExePath = Join-Path $packagePath "tea.exe"
$teaDaemonPath = Join-Path $packagePath "tea-daemon.exe"
$teaCliPath = Join-Path $packagePath "tea-cli.exe"
$teaMcpPath = Join-Path $packagePath "tea-mcp.exe"
$teaSyncPath = Join-Path $packagePath "tea-sync.exe"
$startTeaPath = Join-Path $packagePath "start-tea.bat"
$startTeaDaemonPath = Join-Path $packagePath "start-tea-daemon.bat"
$stopTeaPath = Join-Path $packagePath "stop-tea.bat"
$resolveTeaTokenPath = Join-Path $packagePath "resolve-tea-token.ps1"
$startTeaDaemonScriptPath = Join-Path $packagePath "start-tea-daemon.ps1"
$stopTeaScriptPath = Join-Path $packagePath "stop-tea.ps1"

$packageFiles = @(Get-PackageFilesWithoutReparsePoints -RootPath $packagePath)
foreach ($requiredPath in @(
    $manifestPath,
    $checksumsPath,
    $teaExePath,
    $teaDaemonPath,
    $teaCliPath,
    $teaMcpPath,
    $teaSyncPath,
    $startTeaPath,
    $startTeaDaemonPath,
    $stopTeaPath,
    $resolveTeaTokenPath,
    $startTeaDaemonScriptPath,
    $stopTeaScriptPath
)) {
    Assert-PackagePathContainsNoReparsePoints -Path $requiredPath
}

Assert-True (Test-Path -LiteralPath $manifestPath -PathType Leaf) "Missing manifest.json in package directory."
Assert-True (Test-Path -LiteralPath $checksumsPath -PathType Leaf) "Missing checksums.sha256 in package directory."
Assert-True (Test-Path -LiteralPath $teaExePath -PathType Leaf) "Missing tea.exe UI executable in package directory."
Assert-True (Test-Path -LiteralPath $teaDaemonPath -PathType Leaf) "Missing tea-daemon.exe in package directory."
Assert-True (Test-Path -LiteralPath $teaCliPath -PathType Leaf) "Missing tea-cli.exe in package directory."
Assert-True (Test-Path -LiteralPath $teaMcpPath -PathType Leaf) "Missing tea-mcp.exe MCP server in package directory."
Assert-True (Test-Path -LiteralPath $teaSyncPath -PathType Leaf) "Missing tea-sync.exe sync CLI in package directory."
Assert-True (Test-Path -LiteralPath $startTeaPath -PathType Leaf) "Missing start-tea.bat in package directory."
Assert-True (Test-Path -LiteralPath $startTeaDaemonPath -PathType Leaf) "Missing start-tea-daemon.bat in package directory."
Assert-True (Test-Path -LiteralPath $stopTeaPath -PathType Leaf) "Missing stop-tea.bat in package directory."
Assert-True (Test-Path -LiteralPath $resolveTeaTokenPath -PathType Leaf) "Missing resolve-tea-token.ps1 in package directory."
Assert-True (Test-Path -LiteralPath $startTeaDaemonScriptPath -PathType Leaf) "Missing start-tea-daemon.ps1 in package directory."
Assert-True (Test-Path -LiteralPath $stopTeaScriptPath -PathType Leaf) "Missing stop-tea.ps1 in package directory."

$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$manifestSchemaVersion = [int]$manifest.schemaVersion
Assert-True ($manifestSchemaVersion -in @(1, 2, 3)) "Manifest schemaVersion must be 1, 2, or 3."
Assert-Equal "Tea" $manifest.app "Manifest app must be Tea."
Assert-Equal "Tea" $manifest.sourceProject "Manifest sourceProject must be Tea."
Assert-Equal (Split-Path -Leaf $packagePath) $manifest.versionId "Manifest versionId must match the package directory."
Assert-Equal "release" $manifest.profile "Manifest profile must be release."
Assert-Equal "windows-x64" $manifest.target "Manifest target must be windows-x64."
Assert-Equal "checksums.sha256" $manifest.checksums "Manifest checksums path must be checksums.sha256."
Assert-True ([string]$manifest.gitHead -match '^[0-9a-fA-F]{40}$') "Manifest gitHead must be a full Git commit hash."
Assert-True ([string]$manifest.gitShortSha -match '^[0-9a-fA-F]{8}$') "Manifest gitShortSha must be an 8-character Git commit hash."
Assert-Equal ([string]$manifest.gitHead).Substring(0, 8).ToLowerInvariant() ([string]$manifest.gitShortSha).ToLowerInvariant() "Manifest gitShortSha must match gitHead."
Assert-True ($manifest.gitDirty -is [bool]) "Manifest gitDirty must be a JSON boolean."
if (-not $AllowDirtyManifest) {
    Assert-Equal $false ([bool]$manifest.gitDirty) "Manifest gitDirty must be false for a formal Tea release package."
}

$expectedExeNames = @("tea.exe", "tea-daemon.exe", "tea-cli.exe", "tea-mcp.exe", "tea-sync.exe")
$exeRecords = @($manifest.exes)
$exeNames = @($exeRecords | ForEach-Object { [string]$_.name })
Assert-Equal ($expectedExeNames -join ",") ($exeNames -join ",") "Manifest must list Tea UI, headless daemon, CLI, MCP, and sync executables."
for ($index = 0; $index -lt $expectedExeNames.Count; $index += 1) {
    $expectedName = $expectedExeNames[$index]
    Assert-Equal $expectedName ([string]$exeRecords[$index].path) "Manifest executable path must match its canonical name for $expectedName."
}

$supportNames = @($manifest.supportFiles | ForEach-Object { [string]$_.path })
Assert-Equal "start-tea.bat,start-tea-daemon.bat,stop-tea.bat,resolve-tea-token.ps1,start-tea-daemon.ps1,stop-tea.ps1" ($supportNames -join ",") "Manifest must list every Tea launcher support file."

$expectedZipEntries = @(
    @($manifest.exes) + @($manifest.supportFiles) |
        ForEach-Object { ([string]$_.path).Replace("\", "/") }
)
Assert-Equal $expectedZipEntries.Count @($expectedZipEntries | Sort-Object -Unique -CaseSensitive).Count "Expected Tea ZIP payload paths must be unique."
$expectedZipEntriesOrdinal = @(Sort-OrdinalStrings -Values $expectedZipEntries)

$deterministicZipExpected = $manifestSchemaVersion -ge 2
if ($deterministicZipExpected) {
    Assert-True ($null -ne $manifest.packaging) "Manifest schemaVersion 2 must include packaging metadata."
    Assert-True ($null -ne $manifest.packaging.zip) "Manifest schemaVersion 2 must include ZIP packaging metadata."
    Assert-Equal $true ([bool]$manifest.packaging.zip.deterministic) "Manifest ZIP packaging must be deterministic."
    Assert-Equal "ordinal" ([string]$manifest.packaging.zip.entryOrder) "Manifest ZIP entry order must be ordinal."
    Assert-Equal "1980-01-01T00:00:00" ([string]$manifest.packaging.zip.entryTimestamp) "Manifest ZIP entry timestamp must be canonical."
    Assert-Equal "optimal" ([string]$manifest.packaging.zip.compressionLevel) "Manifest ZIP compression level must be optimal."
    Assert-Equal "same-payload-double-build" ([string]$manifest.packaging.zip.verification) "Manifest ZIP reproducibility verification must be recorded."
}

$binaryRepeatBuildVerified = $false
if ($manifestSchemaVersion -ge 3) {
    Assert-True ($null -ne $manifest.reproducibility) "Manifest schemaVersion 3 must include reproducibility metadata."
    Assert-Equal 1 ([int]$manifest.reproducibility.schemaVersion) "Manifest reproducibility schema version must be 1."
    Assert-True ($null -ne $manifest.reproducibility.binaries) "Manifest must include binary reproducibility metadata."
    Assert-Equal "verified" ([string]$manifest.reproducibility.binaries.status) "Manifest binary repeat-build status must be verified."
    Assert-Equal "same-worktree-repeat-build" ([string]$manifest.reproducibility.binaries.method) "Manifest binary repeat-build method must be explicit."
    Assert-Equal $false ([bool]$manifest.reproducibility.binaries.cleanBuilds) "Manifest must not misrepresent same-worktree repeat builds as clean builds."
    $binaryRecords = @($manifest.reproducibility.binaries.artifacts)
    Assert-Equal $exeNames.Count $binaryRecords.Count "Manifest binary repeat-build records must cover every executable."
    Assert-Equal ($exeNames -join ",") (@($binaryRecords | ForEach-Object { [string]$_.name }) -join ",") "Manifest binary repeat-build record order must match executables."
    foreach ($binaryRecord in $binaryRecords) {
        $name = [string]$binaryRecord.name
        $firstSha256 = [string]$binaryRecord.firstSha256
        $secondSha256 = [string]$binaryRecord.secondSha256
        Assert-Sha256Value -Value $firstSha256 -Context "First repeat-build SHA256 for $name"
        Assert-Sha256Value -Value $secondSha256 -Context "Second repeat-build SHA256 for $name"
        Assert-Equal $firstSha256.ToLowerInvariant() $secondSha256.ToLowerInvariant() "Binary repeat-build hashes must match for $name."
        $exeRecord = @($manifest.exes | Where-Object { [string]$_.name -eq $name })
        Assert-Equal 1 $exeRecord.Count "Binary repeat-build record must identify exactly one executable: $name"
        Assert-Equal ([string]$exeRecord[0].sha256).ToLowerInvariant() $secondSha256.ToLowerInvariant() "Packaged executable must match the verified second build for $name."
        Assert-Equal ([long]$exeRecord[0].bytes) ([long]$binaryRecord.bytes) "Binary repeat-build size must match the packaged executable for $name."
    }
    $binaryRepeatBuildVerified = $true
}

$recordPaths = @(
    @($manifest.exes) +
    @($manifest.supportFiles) +
    @($manifest.buildInfo) +
    @($manifest.buildLogs) +
    @($manifest.artifacts) |
        Where-Object { $null -ne $_ } |
        ForEach-Object { [string]$_.path }
)
Assert-Equal $recordPaths.Count @($recordPaths | Sort-Object -Unique).Count "Manifest record paths must be unique."

foreach ($exe in @($manifest.exes)) {
    Test-HashRecord -RelativePath ([string]$exe.path) -ExpectedSha256 ([string]$exe.sha256)
}

foreach ($supportFile in @($manifest.supportFiles)) {
    Test-HashRecord -RelativePath ([string]$supportFile.path) -ExpectedSha256 ([string]$supportFile.sha256)
}

if ($manifest.buildInfo) {
    Test-HashRecord -RelativePath ([string]$manifest.buildInfo.path) -ExpectedSha256 ([string]$manifest.buildInfo.sha256)
}

foreach ($log in @($manifest.buildLogs)) {
    Test-HashRecord -RelativePath ([string]$log.path) -ExpectedSha256 ([string]$log.sha256)
}

foreach ($artifact in @($manifest.artifacts)) {
    Test-HashRecord -RelativePath ([string]$artifact.path) -ExpectedSha256 ([string]$artifact.sha256)
}

$zipRecords = @($manifest.artifacts | Where-Object { [string]$_.kind -eq "zip" })
Assert-Equal 1 $zipRecords.Count "Manifest must include exactly one zip artifact."
if ($manifestSchemaVersion -ge 3) {
    Assert-True ($null -ne $manifest.reproducibility.zip) "Manifest must include ZIP reproducibility metadata."
    Assert-Equal "verified" ([string]$manifest.reproducibility.zip.status) "Manifest ZIP reproducibility status must be verified."
    Assert-Equal "same-payload-double-build" ([string]$manifest.reproducibility.zip.method) "Manifest ZIP reproducibility method must be explicit."
    Assert-Sha256Value -Value ([string]$manifest.reproducibility.zip.sha256) -Context "Manifest reproducibility ZIP SHA256"
    Assert-Equal ([string]$zipRecords[0].sha256).ToLowerInvariant() ([string]$manifest.reproducibility.zip.sha256).ToLowerInvariant() "Manifest reproducibility ZIP SHA256 must match the ZIP artifact."
}
foreach ($zipRecord in $zipRecords) {
    $zipRelative = [string]$zipRecord.path

    $zipPath = Resolve-PackageFilePath -RelativePath $zipRelative
    $zipShaPath = "$zipPath.sha256"
    Assert-True (Test-Path -LiteralPath $zipShaPath -PathType Leaf) "Missing .zip.sha256 sidecar for $zipRelative."
    $sidecarContent = [System.IO.File]::ReadAllText($zipShaPath)
    $sidecarMatch = [System.Text.RegularExpressions.Regex]::Match(
        $sidecarContent,
        '\A([0-9a-fA-F]{64})  ([^\r\n]+)\r\n\z',
        [System.Text.RegularExpressions.RegexOptions]::CultureInvariant
    )
    Assert-True $sidecarMatch.Success "Zip sidecar must contain exactly one canonical '<64-hex>  <zip-name>' CRLF-terminated record: $zipShaPath"
    $expected = $sidecarMatch.Groups[1].Value.ToLowerInvariant()
    $sidecarName = $sidecarMatch.Groups[2].Value
    Assert-Equal (Split-Path -Leaf $zipPath) $sidecarName "Zip sidecar file name must match the zip artifact."
    $actual = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Equal $expected $actual "Zip sidecar SHA256 mismatch for $zipRelative."

    $zipEntryRecords = @(Get-ZipFileEntries -ZipPath $zipPath)
    $zipEntries = @($zipEntryRecords | ForEach-Object { [string]$_.path })
    Assert-Equal $zipEntries.Count @($zipEntries | Sort-Object -Unique -CaseSensitive).Count "Tea ZIP must not contain duplicate file entries."
    $expectedZipEntryList = $expectedZipEntriesOrdinal -join ","
    $actualZipEntryList = @(Sort-OrdinalStrings -Values $zipEntries) -join ","
    Assert-Equal $expectedZipEntryList $actualZipEntryList "Tea ZIP file entries must exactly match the executable and launcher payload."
    if ($deterministicZipExpected) {
        Assert-Equal $expectedZipEntryList ($zipEntries -join ",") "Tea ZIP entries must be stored in ordinal path order."
        foreach ($entry in $zipEntryRecords) {
            $timestamp = [System.DateTimeOffset]$entry.timestamp
            $timestampValue = $timestamp.ToString(
                "yyyy-MM-ddTHH:mm:ss",
                [System.Globalization.CultureInfo]::InvariantCulture
            )
            Assert-Equal "1980-01-01T00:00:00" $timestampValue "Tea ZIP entry timestamp must be canonical for $($entry.path)."
            Assert-Equal 0 ([int]$entry.externalAttributes) "Tea ZIP entry external attributes must be canonical for $($entry.path)."
        }
    }
}

$checksumEntries = @{}
foreach ($line in @(Get-Content -LiteralPath $checksumsPath)) {
    $checksumMatch = [System.Text.RegularExpressions.Regex]::Match(
        $line,
        '\A([0-9a-fA-F]{64})  (.+)\z',
        [System.Text.RegularExpressions.RegexOptions]::CultureInvariant
    )
    if (-not $checksumMatch.Success) {
        throw "Invalid checksums.sha256 line: $line"
    }
    $relative = $checksumMatch.Groups[2].Value
    Assert-Equal $relative.Trim() $relative "checksums.sha256 paths must not have leading or trailing whitespace."
    $null = Resolve-PackageFilePath -RelativePath $relative
    Assert-True (-not $checksumEntries.ContainsKey($relative)) "checksums.sha256 contains a duplicate path: $relative"
    $checksumEntries[$relative] = $checksumMatch.Groups[1].Value.ToLowerInvariant()
}

$files = $packageFiles |
    Where-Object { $_.FullName -ne $checksumsPath } |
    Sort-Object FullName

foreach ($file in $files) {
    $relative = Get-RelativePath -BasePath $packagePath -Path $file.FullName
    Assert-True $checksumEntries.ContainsKey($relative) "checksums.sha256 missing entry for $relative."
    $actual = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Equal $checksumEntries[$relative] $actual "checksums.sha256 mismatch for $relative."
}
Assert-Equal $files.Count $checksumEntries.Count "checksums.sha256 must contain exactly one entry for every package file."

if ($RunSmoke) {
    $smokePath = Join-Path $repoRoot "scripts\smoke-tea-cli-real.ps1"
    Assert-True (Test-Path -LiteralPath $smokePath -PathType Leaf) "Missing smoke-tea-cli-real.ps1."
    $smokeArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $smokePath, "-PackageDir", $packagePath)
    if ($AllowDirtyManifest) {
        $smokeArgs += "-AllowDirtyManifest"
    }
    & powershell.exe @smokeArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Tea release package smoke failed for $packagePath"
    }

    $uiSmokePath = Join-Path $repoRoot "scripts\smoke-tea-ui-real.ps1"
    Assert-True (Test-Path -LiteralPath $uiSmokePath -PathType Leaf) "Missing smoke-tea-ui-real.ps1."
    $desktopSmokeOutput = @(& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $uiSmokePath -PackageDir $packagePath 2>&1)
    if ($LASTEXITCODE -ne 0) {
        $desktopSmokeOutput | ForEach-Object { Write-Output $_ }
        throw "Tea UI release package smoke failed for $packagePath"
    }

    $mcpSmokePath = Join-Path $repoRoot "scripts\smoke-tea-mcp-real.ps1"
    Assert-True (Test-Path -LiteralPath $mcpSmokePath -PathType Leaf) "Missing smoke-tea-mcp-real.ps1."
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $mcpSmokePath -PackageDir $packagePath
    if ($LASTEXITCODE -ne 0) {
        throw "Tea MCP release package smoke failed for $packagePath"
    }

    $configRecoverySmokePath = Join-Path $repoRoot "scripts\smoke-tea-config-recovery-real.ps1"
    Assert-True (Test-Path -LiteralPath $configRecoverySmokePath -PathType Leaf) "Missing smoke-tea-config-recovery-real.ps1."
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $configRecoverySmokePath -PackageDir $packagePath
    if ($LASTEXITCODE -ne 0) {
        throw "Tea config recovery release package smoke failed for $packagePath"
    }

    $configConcurrencySmokePath = Join-Path $repoRoot "scripts\smoke-tea-config-concurrency-real.ps1"
    Assert-True (Test-Path -LiteralPath $configConcurrencySmokePath -PathType Leaf) "Missing smoke-tea-config-concurrency-real.ps1."
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $configConcurrencySmokePath -PackageDir $packagePath
    if ($LASTEXITCODE -ne 0) {
        throw "Tea config concurrency release package smoke failed for $packagePath"
    }
}

[ordered]@{
    status = "passed"
    packageDir = $packagePath
    versionId = $manifest.versionId
    gitHead = $manifest.gitHead
    gitDirty = [bool]$manifest.gitDirty
    exes = $exeNames
    zipArtifacts = @($zipRecords | ForEach-Object { $_.path })
    zipEntriesValidated = $expectedZipEntries.Count
    deterministicZip = $deterministicZipExpected
    binaryRepeatBuildVerified = $binaryRepeatBuildVerified
    smoke = [bool]$RunSmoke
    allowDirtyManifest = [bool]$AllowDirtyManifest
} | ConvertTo-Json -Depth 6
