[CmdletBinding()]
param(
  [switch]$Offline
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
$LauncherDir = Join-Path $ProjectDir "launcher"
$Distro = "Ubuntu-24.04"

function Invoke-GateStep {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [Parameter(Mandatory = $true)][string]$WorkingDirectory,
    [Parameter(Mandatory = $true)][scriptblock]$Action
  )
  Write-Host "[release-gate] $Name"
  Push-Location $WorkingDirectory
  try {
    & $Action
    if ($LASTEXITCODE -ne 0) {
      throw "$Name failed with exit code $LASTEXITCODE."
    }
  } finally {
    Pop-Location
  }
}

function Resolve-PreparedPython {
  $candidates = @(
    (Join-Path $ProjectDir ".venv\Scripts\python.exe"),
    $env:PYTHON,
    $(
      $command = Get-Command python -ErrorAction SilentlyContinue
      if ($command) { $command.Source }
    )
  ) | Where-Object { $_ } | Select-Object -Unique

  $rejected = @()
  foreach ($candidate in $candidates) {
    if (-not (Test-Path -LiteralPath $candidate)) {
      $rejected += "$candidate (not found)"
      continue
    }
    $preflight = @(& $candidate -c "import sys; import fastapi, httpx, starlette, uvicorn, pytest; print('CSA_RELEASE_PYTHON_' + str(sys.version_info[0]))" 2>$null)
    if ($LASTEXITCODE -eq 0 -and "CSA_RELEASE_PYTHON_3" -in $preflight) {
      return (Resolve-Path -LiteralPath $candidate).Path
    }
    $rejected += "$candidate (missing locked test/runtime dependencies)"
  }

  $detail = if ($rejected.Count) { $rejected -join "; " } else { "no interpreter candidates" }
  throw "Release gate requires a prepared Python environment. Create .venv and install requirements-dev.txt before running the offline gate. Checked: $detail"
}

$python = Resolve-PreparedPython
Write-Host "[release-gate] prepared Python: $python"
$cargo = (Get-Command cargo -ErrorAction Stop).Source
$npm = (Get-Command npm.cmd -ErrorAction Stop).Source
if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
  throw "Release gate requires WSL2 and Ubuntu-24.04 for lifecycle fault tests."
}
$distros = @(((& wsl.exe --list --quiet 2>$null) -replace [char]0, "") | ForEach-Object { $_.Trim() })
if ($Distro -notin $distros) {
  throw "Release gate requires the tested distro $Distro."
}

$savedEnvironment = @{}
foreach ($name in @("PIP_NO_INDEX", "CARGO_NET_OFFLINE", "npm_config_offline")) {
  $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
}
try {
  if ($Offline) {
    $env:PIP_NO_INDEX = "1"
    $env:CARGO_NET_OFFLINE = "true"
    $env:npm_config_offline = "true"
  }

  Invoke-GateStep "full package self-test" $ProjectDir {
    $selfTestParameters = @{ Python = $python }
    if ($Offline) { $selfTestParameters.Offline = $true }
    & (Join-Path $ProjectDir "scripts\self-test.ps1") @selfTestParameters
  }
  Invoke-GateStep "frontend production build" $LauncherDir {
    & $npm run build
  }
  Invoke-GateStep "Rust unit/integration tests" (Join-Path $LauncherDir "src-tauri") {
    $arguments = @("test", "--jobs", "1")
    if ($Offline) { $arguments += "--offline" }
    & $cargo @arguments
  }

  $projectWsl = ((& wsl.exe -d $Distro -- wslpath -a $ProjectDir.Replace("\", "/") 2>$null) -replace [char]0, "").Trim()
  if (-not $projectWsl.StartsWith("/")) {
    throw "Could not convert the project path to WSL."
  }
  Invoke-GateStep "WSL shell syntax" $ProjectDir {
    $syntaxFiles = @(
      "$projectWsl/scripts/csa-runtime-layout.sh"
      "$projectWsl/scripts/start-claude-science-wsl.sh"
      "$projectWsl/scripts/install-wsl-bridge-service.sh"
      "$projectWsl/skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh"
      "$projectWsl/tests/runtime_layout_test.sh"
      "$projectWsl/tests/runtime_activation_integration_test.sh"
      "$projectWsl/tests/runtime_network_contract_functions_test.sh"
      "$projectWsl/tests/runtime_lifecycle_20cycle_test.sh"
    )
    & wsl.exe -d $Distro -- bash -n @syntaxFiles
  }
  Invoke-GateStep "WSL runtime layout" $ProjectDir {
    & wsl.exe -d $Distro -- bash "$projectWsl/tests/runtime_layout_test.sh" "$projectWsl"
  }
  Invoke-GateStep "WSL activation rollback and proxy poisoning" $ProjectDir {
    & wsl.exe -d $Distro -- bash "$projectWsl/tests/runtime_activation_integration_test.sh" "$projectWsl"
  }
  Invoke-GateStep "WSL runtime network contract functions" $ProjectDir {
    & wsl.exe -d $Distro -- bash "$projectWsl/tests/runtime_network_contract_functions_test.sh" "$projectWsl"
  }
  Invoke-GateStep "WSL 20-cycle lifecycle" $ProjectDir {
    & wsl.exe -d $Distro -- bash "$projectWsl/tests/runtime_lifecycle_20cycle_test.sh" "$projectWsl"
  }
} finally {
  foreach ($name in $savedEnvironment.Keys) {
    [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], "Process")
  }
}

Write-Host "release gate passed"
