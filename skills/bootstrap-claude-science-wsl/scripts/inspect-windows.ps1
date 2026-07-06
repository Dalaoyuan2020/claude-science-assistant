[CmdletBinding()]
param(
  [string]$ProjectRoot = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Try-Value {
  param([scriptblock]$Action, $Fallback = $null)
  try { & $Action } catch { $Fallback }
}

function Get-Listener {
  param([int]$Port)
  $connections = @(Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)
  @($connections | ForEach-Object {
    $process = Get-CimInstance Win32_Process -Filter "ProcessId=$($_.OwningProcess)" -ErrorAction SilentlyContinue
    [ordered]@{
      address = $_.LocalAddress
      port = $_.LocalPort
      pid = $_.OwningProcess
      process = if ($process) { $process.Name } else { "unknown" }
      managed_bridge = [bool]($process -and $process.CommandLine -match 'claude-science-api-bridge.*proxy\.py|proxy\.py')
    }
  })
}

function Decode-WslLines {
  $raw = (& wsl.exe --list --quiet 2>$null) -replace "`0", ""
  @($raw | ForEach-Object { $_.Trim() } | Where-Object { $_ -and $_ -notmatch '^docker-desktop' })
}

$computer = Get-CimInstance Win32_OperatingSystem
$processor = Get-CimInstance Win32_Processor | Select-Object -First 1
$systemDrive = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='$($env:SystemDrive)'"
$distros = @(Try-Value { Decode-WslLines } @())
$recommendedDistro = 'Ubuntu-24.04'
$preferredDistro = @($distros | Where-Object { $_ -eq $recommendedDistro } | Select-Object -First 1)
if (-not $preferredDistro.Count) {
  $preferredDistro = @($distros | Where-Object { $_ -match '^Ubuntu' } | Select-Object -First 1)
}
if (-not $preferredDistro.Count) {
  $preferredDistro = @($distros | Select-Object -First 1)
}

$wslStatusText = Try-Value { ((& wsl.exe --status 2>&1) -replace "`0", "") -join "`n" } ""
$pendingReboot = [bool](
  (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending') -or
  (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired')
)

$project = $null
if ($ProjectRoot) {
  $resolved = Try-Value { (Resolve-Path -LiteralPath $ProjectRoot).Path } ""
  $launcherDev = [bool]($resolved -and (Test-Path (Join-Path $resolved 'launcher\src-tauri\tauri.conf.json')))
  $launcherPortable = [bool]($resolved -and (Test-Path (Join-Path $resolved 'claude-science-assistant.exe')))
  $project = [ordered]@{
    path_exists = [bool]$resolved
    proxy_exists = [bool]($resolved -and (Test-Path (Join-Path $resolved 'proxy.py')))
    config_exists = [bool]($resolved -and (Test-Path (Join-Path $resolved 'config.json')))
    launcher_exists = [bool]($launcherDev -or $launcherPortable)
  }
}

$ports = [ordered]@{}
foreach ($port in @(9876, 9877, 8765, 8766, 443)) {
  $ports[[string]$port] = @(Get-Listener -Port $port)
}

$windowsBridgePids = @($ports['9876'] | Where-Object { $_.managed_bridge } | ForEach-Object { $_.pid })
$report = [ordered]@{
  schema_version = 1
  generated_at = (Get-Date).ToUniversalTime().ToString('o')
  mode = 'read-only'
  platform = [ordered]@{
    product_name = $computer.Caption
    build = $computer.BuildNumber
    architecture = $computer.OSArchitecture
    supported = [bool]([int]$computer.BuildNumber -ge 19045 -and $computer.OSArchitecture -match '64')
    virtualization_firmware_enabled = [bool]$processor.VirtualizationFirmwareEnabled
    free_system_drive_gb = [math]::Round($systemDrive.FreeSpace / 1GB, 1)
    pending_reboot = $pendingReboot
  }
  wsl = [ordered]@{
    command_available = [bool](Get-Command wsl.exe -ErrorAction SilentlyContinue)
    status_available = [bool]$wslStatusText
    distros = $distros
    preferred_distro = if ($preferredDistro.Count) { $preferredDistro[0] } else { $null }
    recommended_distro = $recommendedDistro
    compatibility_mode = 'recommend-ubuntu-24.04-compatible-with-installed-wsl'
    locked_distro = $null
    locked_distro_present = $false
  }
  runtime = [ordered]@{
    user_anthropic_base_url_set = [bool][Environment]::GetEnvironmentVariable('ANTHROPIC_BASE_URL', 'User')
    scheduled_task_present = [bool](Get-ScheduledTask -TaskName 'ClaudeScienceByokProxy' -ErrorAction SilentlyContinue)
    windows_bridge_pids = $windowsBridgePids
    duplicate_bridge_warning = [bool]($windowsBridgePids.Count -gt 0 -and $distros.Count -gt 0)
    ports = $ports
  }
  project = $project
  secrets = [ordered]@{
    values_included = $false
  }
}

$report | ConvertTo-Json -Depth 8 -Compress
