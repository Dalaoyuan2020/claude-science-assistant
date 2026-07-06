[CmdletBinding()]
param(
  [string]$ProjectRoot = "",
  [string]$Distro = "",
  [string]$User = "",
  [int]$ProxyPort = 9876,
  [switch]$PlanOnly,
  [switch]$ApproveInstall,
  [switch]$InstallWslIfMissing,
  [switch]$StartServices,
  [switch]$RunSelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $ProjectRoot) {
  $ProjectRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..\..\..")).Path
} else {
  $ProjectRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
}

function Write-Step {
  param([string]$Message)
  Write-Host ""
  Write-Host "== $Message =="
}

function Is-Admin {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = [Security.Principal.WindowsPrincipal]::new($identity)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-Distros {
  $raw = ((& wsl.exe --list --quiet 2>$null) -replace [char]0, "")
  @($raw | ForEach-Object { $_.Trim() } | Where-Object { $_ -and $_ -notmatch '^docker-desktop' })
}

function Select-Distro {
  param([string[]]$Distros)
  $recommended = @($Distros | Where-Object { $_ -eq 'Ubuntu-24.04' } | Select-Object -First 1)
  if ($recommended.Count) { return $recommended[0] }
  $ubuntu = @($Distros | Where-Object { $_ -match '^Ubuntu' } | Select-Object -First 1)
  if ($ubuntu.Count) { return $ubuntu[0] }
  if ($Distros.Count) { return $Distros[0] }
  return $null
}

function Invoke-Wsl {
  param([string[]]$Command)
  $wslArgs = @("-d", $Distro)
  if ($User) { $wslArgs += @("-u", $User) }
  $wslArgs += "--"
  & wsl.exe @wslArgs @Command
}

function Get-WslPath {
  param([string]$WindowsPath)
  $converted = ((Invoke-Wsl @("wslpath", "-a", $WindowsPath)) -replace [char]0, "")
  return ($converted | Select-Object -First 1).Trim()
}

Write-Step "Read-only Windows inspection"
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "inspect-windows.ps1") -ProjectRoot $ProjectRoot

if (-not $PlanOnly -and -not $ApproveInstall) {
  throw "Refusing to change this PC without -ApproveInstall. Rerun with -PlanOnly to preview or -ApproveInstall after user confirmation."
}

if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
  throw "wsl.exe was not found. Install/enable WSL from an elevated Windows terminal, then rerun inspection."
}

$distros = @(Get-Distros)
if (-not $Distro) {
  $Distro = Select-Distro -Distros $distros
}
if (-not $Distro) {
  $Distro = "Ubuntu-24.04"
}
if ($distros -notcontains $Distro) {
  if ($PlanOnly -or -not $InstallWslIfMissing) {
    Write-Step "Required install action"
    Write-Host "No compatible WSL distro was found. Ubuntu-24.04 is recommended; pass -Distro to target another installed distro."
    Write-Host "After user confirmation, run this script with -ApproveInstall -InstallWslIfMissing to install Ubuntu-24.04."
    Write-Host "This may enable Windows features, download Ubuntu, and require a reboot."
    exit 2
  }
  if (-not (Is-Admin)) {
    throw "Installing WSL/Ubuntu requires an elevated PowerShell window."
  }
  Write-Step "Install WSL distro"
  & wsl.exe --install -d $Distro
  Write-Host "WSL installation was requested. If Windows asks for a reboot, reboot and rerun the inspection."
  exit 3010
}

if (-not $User) {
  $User = (((& wsl.exe -d $Distro -- id -un) -replace [char]0, "") | Select-Object -First 1).Trim()
}

$projectWsl = Get-WslPath $ProjectRoot
$skillRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
$skillWsl = Get-WslPath $skillRoot
$dryRun = if ($PlanOnly) { "1" } else { "0" }
$start = if ($StartServices) { "1" } else { "0" }

Write-Step $(if ($PlanOnly) { "Preview WSL runtime bootstrap" } else { "Apply WSL runtime bootstrap" })
$envVars = @(
  "DRY_RUN=$dryRun",
  "START_SERVICES=$start",
  "PROXY_PORT=$ProxyPort"
)
$bootstrapCommand = @("env") + $envVars + @("bash", "$skillWsl/scripts/bootstrap-wsl-runtime.sh", $projectWsl)
Invoke-Wsl $bootstrapCommand

Write-Step "Read-only WSL inspection"
Invoke-Wsl @("bash", "$skillWsl/scripts/inspect-wsl.sh")

if ($RunSelfTest -and -not $PlanOnly) {
  Write-Step "Project self-test"
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ProjectRoot "scripts\self-test.ps1")
}

Write-Step "Done"
if ($PlanOnly) {
  Write-Host "Plan preview complete. No changes were applied."
} else {
  Write-Host "Repair/bootstrap complete. Review the inspection output before claiming readiness."
}
