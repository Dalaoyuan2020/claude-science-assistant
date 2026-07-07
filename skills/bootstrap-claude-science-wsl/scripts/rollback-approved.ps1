[CmdletBinding()]
param(
  [string]$ProjectRoot = "",
  [string]$Distro = "",
  [string]$User = "",
  [switch]$PlanOnly,
  [switch]$ApproveUninstall,
  [switch]$DeleteProductData
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
  $portablePath = (Resolve-Path -LiteralPath $WindowsPath).Path.Replace("\", "/")
  $converted = @((Invoke-Wsl @("wslpath", "-a", $portablePath)) -replace [char]0, "")
  $candidate = @(
    $converted |
      ForEach-Object { "$_".Trim() } |
      Where-Object { $_ -match '^/' } |
      Select-Object -First 1
  )
  if (-not $candidate.Count) {
    throw "Failed to convert Windows path to WSL path: $WindowsPath"
  }
  return $candidate[0]
}

Write-Step "Read-only Windows inspection"
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "inspect-windows.ps1") -ProjectRoot $ProjectRoot

if (-not $PlanOnly -and -not $ApproveUninstall) {
  throw "Refusing to uninstall without -ApproveUninstall. Rerun with -PlanOnly to preview."
}

if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
  Write-Host "wsl.exe was not found; no WSL runtime exists to roll back."
  exit 0
}

$distros = @(Get-Distros)
if (-not $Distro) {
  $Distro = Select-Distro -Distros $distros
}
if ($distros -notcontains $Distro) {
  $fallback = @($distros | Where-Object { $_ -match 'Ubuntu' } | Select-Object -First 1)
  if ($fallback.Count -gt 0) {
    Write-Host "Requested distro '$Distro' was not found; using '$($fallback[0])'."
    $Distro = $fallback[0]
  } else {
    Write-Host "No supported Ubuntu distro was found; no WSL runtime exists to roll back."
    exit 0
  }
}

if (-not $User) {
  $User = (((& wsl.exe -d $Distro -- id -un) -replace [char]0, "") | Select-Object -First 1).Trim()
}

$skillRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
$skillWsl = Get-WslPath $skillRoot
$dryRun = if ($PlanOnly) { "1" } else { "0" }
$deleteData = if ($DeleteProductData) { "1" } else { "0" }

Write-Step $(if ($PlanOnly) { "Preview WSL runtime rollback" } else { "Apply WSL runtime rollback" })
$rollbackCommand = @(
  "env",
  "DRY_RUN=$dryRun",
  "DELETE_PRODUCT_DATA=$deleteData",
  "bash",
  "$skillWsl/scripts/rollback-wsl-runtime.sh"
)
Invoke-Wsl $rollbackCommand

Write-Step "Read-only WSL inspection"
Invoke-Wsl @("bash", "$skillWsl/scripts/inspect-wsl.sh")

Write-Step "Done"
if ($PlanOnly) {
  Write-Host "Rollback preview complete. No changes were applied."
} else {
  Write-Host "Rollback complete. Provider config, API keys, OAuth tokens, and original Claude Science binary were preserved."
}
