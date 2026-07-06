@echo off
setlocal EnableExtensions
chcp 65001 >nul

set "NO_PAUSE="
if /I "%~1"=="--no-pause" (
  set "NO_PAUSE=1"
  shift /1
)

set FORWARD_ARGS=
:collect_args
if "%~1"=="" goto args_ready
set FORWARD_ARGS=%FORWARD_ARGS% "%~1"
shift /1
goto collect_args
:args_ready

set "SCRIPT_DIR=%~dp0"
set "PS1=%SCRIPT_DIR%acceptance-v0.1.ps1"

echo Claude Science Assistant v0.1 acceptance preview
echo This wrapper keeps the PowerShell script and runs it with ExecutionPolicy Bypass.
echo.

if not exist "%PS1%" (
  echo Missing script: "%PS1%"
  set "EXIT_CODE=1"
  goto done
)

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%PS1%" %FORWARD_ARGS%
set "EXIT_CODE=%ERRORLEVEL%"

:done
echo.
if "%EXIT_CODE%"=="0" (
  echo Finished successfully.
) else (
  echo Failed with exit code %EXIT_CODE%.
)

if not defined NO_PAUSE (
  echo.
  pause
)

exit /b %EXIT_CODE%
