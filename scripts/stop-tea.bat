@echo off
setlocal

set "TEA_HOME=%~dp0"
if "%TEA_HOME:~-1%"=="\" set "TEA_HOME=%TEA_HOME:~0,-1%"
set "TEA_DATA_DIR=%TEA_HOME%\data"

powershell -NoProfile -ExecutionPolicy Bypass -Command "$home = (Resolve-Path -LiteralPath $env:TEA_HOME).Path; $daemonExe = Join-Path $home 'tea-daemon.exe'; $uiExe = Join-Path $home 'tea.exe'; $pidFile = Join-Path $env:TEA_DATA_DIR 'tea-daemon.pid'; if (Test-Path -LiteralPath $pidFile) { $pidValue = (Get-Content -Raw -LiteralPath $pidFile).Trim(); if ($pidValue -match '^\d+$') { $p = Get-Process -Id ([int]$pidValue) -ErrorAction SilentlyContinue; if ($p -and $p.Path -eq $daemonExe) { Stop-Process -Id $p.Id -Force; Write-Host ('stopped tea-daemon pid=' + $p.Id) } }; Remove-Item -LiteralPath $pidFile -Force -ErrorAction SilentlyContinue }; Get-Process -Name 'tea' -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $uiExe } | ForEach-Object { Stop-Process -Id $_.Id -Force; Write-Host ('stopped tea ui pid=' + $_.Id) }"
endlocal
