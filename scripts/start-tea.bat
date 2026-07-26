@echo off
setlocal

set "TEA_HOME=%~dp0"
if "%TEA_HOME:~-1%"=="\" set "TEA_HOME=%TEA_HOME:~0,-1%"
set "TEA_DATA_DIR=%TEA_HOME%\data"
set "TEA_LOG_DIR=%TEA_HOME%\logs"
if not exist "%TEA_DATA_DIR%" mkdir "%TEA_DATA_DIR%"
if not exist "%TEA_LOG_DIR%" mkdir "%TEA_LOG_DIR%"

if not defined TEA_AUTH_TOKEN (
  for /f "usebackq delims=" %%T in (`powershell -NoProfile -ExecutionPolicy Bypass -Command "$p = Join-Path $env:TEA_DATA_DIR 'auth-token.txt'; if (-not (Test-Path -LiteralPath $p)) { [System.IO.File]::WriteAllText($p, [guid]::NewGuid().ToString('N'), [System.Text.UTF8Encoding]::new($false)) }; (Get-Content -Raw -LiteralPath $p).Trim()"`) do set "TEA_AUTH_TOKEN=%%T"
)
if not defined TEA_BIND_ADDR set "TEA_BIND_ADDR=127.0.0.1:48910"
if not defined TEA_SERVER_URL set "TEA_SERVER_URL=http://127.0.0.1:48910"
if not defined TEA_STORE_PATH set "TEA_STORE_PATH=%TEA_DATA_DIR%\tea.sqlite"
if not defined TEA_CONFIG_PATH set "TEA_CONFIG_PATH=%TEA_DATA_DIR%\config.json"

powershell -NoProfile -ExecutionPolicy Bypass -Command "try { Invoke-RestMethod -Uri ($env:TEA_SERVER_URL.TrimEnd('/') + '/health') -TimeoutSec 1 | Out-Null; exit 0 } catch { exit 1 }"
if errorlevel 1 (
  powershell -NoProfile -ExecutionPolicy Bypass -Command "$out = Join-Path $env:TEA_LOG_DIR 'tea-daemon.out.log'; $err = Join-Path $env:TEA_LOG_DIR 'tea-daemon.err.log'; $exe = Join-Path $env:TEA_HOME 'tea-daemon.exe'; $args = @('--bind-addr', $env:TEA_BIND_ADDR, '--auth-token', $env:TEA_AUTH_TOKEN, '--store-path', $env:TEA_STORE_PATH, '--config-path', $env:TEA_CONFIG_PATH); $p = Start-Process -FilePath $exe -ArgumentList $args -PassThru -WindowStyle Hidden -RedirectStandardOutput $out -RedirectStandardError $err; Set-Content -LiteralPath (Join-Path $env:TEA_DATA_DIR 'tea-daemon.pid') -Value $p.Id -Encoding ASCII"
)

powershell -NoProfile -ExecutionPolicy Bypass -Command "$ok = $false; for ($i = 0; $i -lt 40; $i++) { try { Invoke-RestMethod -Uri ($env:TEA_SERVER_URL.TrimEnd('/') + '/health') -TimeoutSec 1 | Out-Null; $ok = $true; break } catch { Start-Sleep -Milliseconds 250 } }; if (-not $ok) { Write-Error 'Tea daemon did not become healthy.'; exit 1 }"
if errorlevel 1 exit /b 1

start "" "%TEA_HOME%\tea.exe"
endlocal
