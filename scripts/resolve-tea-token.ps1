[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$TokenPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Assert-TeaTokenPathComponent {
    param(
        [string]$Path,
        [bool]$ExpectDirectory,
        [string]$Description
    )

    $item = Get-Item -Force -LiteralPath $Path -ErrorAction SilentlyContinue
    if ($null -eq $item) { return }
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Tea auth token $Description must not be a symlink or reparse point: $Path"
    }
    $typeMatches = if ($ExpectDirectory) { $item.PSIsContainer } else { -not $item.PSIsContainer }
    if (-not $typeMatches) {
        throw "Tea auth token $Description has the wrong filesystem type: $Path"
    }
}

function Assert-TeaTokenPathSafe {
    param([string]$Path)

    Assert-TeaTokenPathComponent -Path (Split-Path -Parent $Path) -ExpectDirectory $true -Description "directory"
    Assert-TeaTokenPathComponent -Path $Path -ExpectDirectory $false -Description "file"
}

$resolvedPath = [System.IO.Path]::GetFullPath($TokenPath)
$parent = Split-Path -Parent $resolvedPath
Assert-TeaTokenPathComponent -Path $parent -ExpectDirectory $true -Description "directory"
New-Item -ItemType Directory -Force -Path $parent | Out-Null

for ($attempt = 0; $attempt -lt 50; $attempt++) {
    Assert-TeaTokenPathSafe -Path $resolvedPath
    if (Test-Path -LiteralPath $resolvedPath -PathType Leaf) {
        try {
            $existing = ([System.IO.File]::ReadAllText($resolvedPath)).Trim()
            if ($existing -match '^[0-9a-fA-F]{32}$') {
                Write-Output $existing
                return
            }
            if (-not [string]::IsNullOrWhiteSpace($existing)) {
                throw "Tea auth token file has an invalid format: $resolvedPath"
            }
        } catch [System.IO.IOException] {}
    } else {
        $token = [Guid]::NewGuid().ToString("N")
        $stream = $null
        try {
            $stream = [System.IO.File]::Open(
                $resolvedPath,
                [System.IO.FileMode]::CreateNew,
                [System.IO.FileAccess]::Write,
                [System.IO.FileShare]::Read
            )
            $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($token)
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush($true)
            Write-Output $token
            return
        } catch [System.IO.IOException] {
            # Another launcher won the create-new race. Read its token next pass.
        } finally {
            if ($null -ne $stream) { $stream.Dispose() }
        }
    }
    Start-Sleep -Milliseconds 20
}

throw "Tea auth token file stayed empty or unreadable: $resolvedPath"
