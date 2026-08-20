@echo off
setlocal

set "TEA_HOME=%~dp0"
if "%TEA_HOME:~-1%"=="\" set "TEA_HOME=%TEA_HOME:~0,-1%"
set "TEA_DATA_DIR=%TEA_HOME%\data"
set "TEA_LOG_DIR=%TEA_HOME%\logs"
if not exist "%TEA_DATA_DIR%" mkdir "%TEA_DATA_DIR%"
if not exist "%TEA_LOG_DIR%" mkdir "%TEA_LOG_DIR%"

if not defined TEA_AUTH_TOKEN (
  for /f "usebackq delims=" %%T in (`powershell -NoProfile -ExecutionPolicy Bypass -File "%TEA_HOME%\resolve-tea-token.ps1" -TokenPath "%TEA_DATA_DIR%\auth-token.txt"`) do set "TEA_AUTH_TOKEN=%%T"
)
if not defined TEA_AUTH_TOKEN exit /b 1
if not defined TEA_BIND_ADDR set "TEA_BIND_ADDR=127.0.0.1:48910"
if not defined TEA_SERVER_URL set "TEA_SERVER_URL=http://127.0.0.1:48910"
if not defined TEA_STORE_PATH set "TEA_STORE_PATH=%TEA_DATA_DIR%\tea.sqlite"
if not defined TEA_CONFIG_PATH set "TEA_CONFIG_PATH=%TEA_DATA_DIR%\config.json"

powershell -NoProfile -ExecutionPolicy Bypass -File "%TEA_HOME%\start-tea-daemon.ps1"
if errorlevel 1 exit /b 1
endlocal
