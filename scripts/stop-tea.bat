@echo off
setlocal

set "TEA_HOME=%~dp0"
if "%TEA_HOME:~-1%"=="\" set "TEA_HOME=%TEA_HOME:~0,-1%"
powershell -NoProfile -ExecutionPolicy Bypass -File "%TEA_HOME%\stop-tea.ps1"
if errorlevel 1 exit /b 1
endlocal
