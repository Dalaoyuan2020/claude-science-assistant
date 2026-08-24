[CmdletBinding()]
param(
  [string]$Only = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
$TauriDir = Join-Path $ProjectDir "launcher\src-tauri"
$Executable = Join-Path $ProjectDir "launcher\src-tauri\target\debug\claude-science-assistant.exe"

Push-Location -LiteralPath $TauriDir
try {
  & cargo build --bin claude-science-assistant
  $BuildExitCode = $LASTEXITCODE
} finally {
  Pop-Location
}

if ($BuildExitCode -ne 0) {
  exit $BuildExitCode
}
if (-not (Test-Path -LiteralPath $Executable)) {
  throw "Debug launcher build completed without the expected executable: $Executable"
}

$SmokeArgs = @("--smoke")
if ($Only) {
  $SmokeArgs += @("--only", $Only)
}

& $Executable @SmokeArgs
exit $LASTEXITCODE
