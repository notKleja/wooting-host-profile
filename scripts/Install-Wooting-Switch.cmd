@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install-Wooting-Switch.ps1"
if errorlevel 1 (
  echo.
  echo Installation failed. Review the message above, then try again.
  pause
)
