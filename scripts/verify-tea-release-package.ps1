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

    $path = Join-Path $packagePath $RelativePath
    Assert-True (Test-Path -LiteralPath $path -PathType Leaf) "Package file missing: $RelativePath"
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Equal $ExpectedSha256.ToLowerInvariant() $actual "SHA256 mismatch for $RelativePath."
}

$manifestPath = Join-Path $packagePath "manifest.json"
$checksumsPath = Join-Path $packagePath "checksums.sha256"
$teaExePath = Join-Path $packagePath "tea.exe"
$teaDaemonPath = Join-Path $packagePath "tea-daemon.exe"
$teaCliPath = Join-Path $packagePath "tea-cli.exe"
$teaMcpPath = Join-Path $packagePath "tea-mcp.exe"
$teaSyncPath = Join-Path $packagePath "tea-sync.exe"
$teaSyncPath = Join-Path $packagePath "tea-sync.exe"
$startTeaPath = Join-Path $packagePath "start-tea.bat"
$startTeaDaemonPath = Join-Path $packagePath "start-tea-daemon.bat"
$stopTeaPath = Join-Path $packagePath "stop-tea.bat"

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

$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
Assert-Equal "Tea" $manifest.app "Manifest app must be Tea."
Assert-Equal "release" $manifest.profile "Manifest profile must be release."
Assert-Equal "windows-x64" $manifest.target "Manifest target must be windows-x64."
if (-not $AllowDirtyManifest) {
    Assert-Equal $false ([bool]$manifest.gitDirty) "Manifest gitDirty must be false for a formal Tea release package."
}

$exeNames = @($manifest.exes | ForEach-Object { [string]$_.name })
Assert-Equal "tea.exe,tea-daemon.exe,tea-cli.exe,tea-mcp.exe,tea-sync.exe" ($exeNames -join ",") "Manifest must list Tea UI, headless daemon, CLI, MCP, and sync executables."

$supportNames = @($manifest.supportFiles | ForEach-Object { [string]$_.path })
Assert-Equal "start-tea.bat,start-tea-daemon.bat,stop-tea.bat" ($supportNames -join ",") "Manifest must list Tea launcher support files."

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

$zipRecords = @($manifest.artifacts | Where-Object { [string]$_.kind -eq "zip" })
Assert-True ($zipRecords.Count -ge 1) "Manifest must include a zip artifact."
foreach ($zipRecord in $zipRecords) {
    $zipRelative = [string]$zipRecord.path
    Test-HashRecord -RelativePath $zipRelative -ExpectedSha256 ([string]$zipRecord.sha256)

    $zipPath = Join-Path $packagePath $zipRelative
    $zipShaPath = "$zipPath.sha256"
    Assert-True (Test-Path -LiteralPath $zipShaPath -PathType Leaf) "Missing .zip.sha256 sidecar for $zipRelative."
    $expected = ((Get-Content -Raw -LiteralPath $zipShaPath).Trim() -split "\s+")[0].ToLowerInvariant()
    $actual = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Equal $expected $actual "Zip sidecar SHA256 mismatch for $zipRelative."
}

$checksumEntries = @{}
Get-Content -LiteralPath $checksumsPath | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object {
    $parts = $_ -split "\s+", 2
    if ($parts.Count -ne 2) {
        throw "Invalid checksums.sha256 line: $_"
    }
    $checksumEntries[$parts[1].Trim()] = $parts[0].ToLowerInvariant()
}

$files = Get-ChildItem -LiteralPath $packagePath -Recurse -File |
    Where-Object { $_.FullName -ne $checksumsPath } |
    Sort-Object FullName

foreach ($file in $files) {
    $relative = Get-RelativePath -BasePath $packagePath -Path $file.FullName
    Assert-True $checksumEntries.ContainsKey($relative) "checksums.sha256 missing entry for $relative."
    $actual = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    Assert-Equal $checksumEntries[$relative] $actual "checksums.sha256 mismatch for $relative."
}

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
}

[ordered]@{
    status = "passed"
    packageDir = $packagePath
    versionId = $manifest.versionId
    gitHead = $manifest.gitHead
    gitDirty = [bool]$manifest.gitDirty
    exes = $exeNames
    zipArtifacts = @($zipRecords | ForEach-Object { $_.path })
    smoke = [bool]$RunSmoke
    allowDirtyManifest = [bool]$AllowDirtyManifest
} | ConvertTo-Json -Depth 6
