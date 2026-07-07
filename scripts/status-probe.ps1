[CmdletBinding()]
param(
  [string]$ProjectRoot = "",
  [string]$Distro = "",
  [string]$User = "",
  [int]$ProxyPort = 9876,
  [switch]$IncludeWindowsPorts
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $ProjectRoot) {
  $ProjectRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
} else {
  $ProjectRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
}

function Try-Value {
  param([scriptblock]$Action, $Fallback = $null)
  try { & $Action } catch { $Fallback }
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

function Convert-ToWslPath {
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

function Get-WindowsListeners {
  param([int[]]$Ports)
  $result = [ordered]@{}
  foreach ($port in $Ports) {
    $result[[string]$port] = @(
      Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue |
        ForEach-Object {
          $process = Get-CimInstance Win32_Process -Filter "ProcessId=$($_.OwningProcess)" -ErrorAction SilentlyContinue
          [ordered]@{
            address = $_.LocalAddress
            port = $_.LocalPort
            pid = $_.OwningProcess
            process = if ($process) { $process.Name } else { "unknown" }
            command = if ($process) { $process.CommandLine } else { "" }
          }
        }
    )
  }
  return $result
}

$warnings = New-Object System.Collections.Generic.List[string]
$distros = @()
if (Get-Command wsl.exe -ErrorAction SilentlyContinue) {
  $distros = @(Get-Distros)
}
if (-not $Distro) {
  $Distro = Select-Distro -Distros $distros
}

$projectExists = (Test-Path -LiteralPath (Join-Path $ProjectRoot "proxy.py")) -and
  (Test-Path -LiteralPath (Join-Path $ProjectRoot "scripts\start-claude-science-wsl.sh"))

$projectWsl = ""
$wslProbe = $null
$probeError = ""
if (-not $distros.Count -or -not $Distro) {
  $warnings.Add("No usable WSL distro was detected.")
} elseif (-not $projectExists) {
  $warnings.Add("ProjectRoot is not a complete CSA package root.")
} else {
  if (-not $User) {
    $User = (((& wsl.exe -d $Distro -- id -un) -replace [char]0, "") | Select-Object -First 1).Trim()
  }
  try {
    $projectWsl = Convert-ToWslPath $ProjectRoot
    $inspectWsl = "$projectWsl/skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh"
    $rawProbe = ((Invoke-Wsl @("env", "PROJECT_DIR=$projectWsl", "PROXY_PORT=$ProxyPort", "bash", $inspectWsl, $projectWsl)) -replace [char]0, "")
    $jsonLine = @(
      $rawProbe |
        ForEach-Object { "$_".Trim() } |
        Where-Object { $_ -match '^\{' } |
        Select-Object -Last 1
    )
    if ($jsonLine.Count) {
      $wslProbe = $jsonLine[0] | ConvertFrom-Json
    } else {
      $probeError = ($rawProbe -join "`n").Trim()
    }
  } catch {
    $probeError = $_.Exception.Message
  }
}

if ($Distro -and $Distro -ne "Ubuntu-24.04" -and $Distro -match '^Ubuntu') {
  $warnings.Add("Ubuntu-24.04 is recommended, but $Distro is treated as a compatible WSL2 candidate.")
}
if ($wslProbe -and $null -ne $wslProbe.runtime.unit_matches_project -and -not $wslProbe.runtime.unit_matches_project) {
  $warnings.Add("claude-science-bridge.service does not point to the current ProjectRoot.")
}
if ($probeError) {
  $warnings.Add("WSL status probe failed: $probeError")
}

$bridgeHealthy = [bool]($wslProbe -and $wslProbe.runtime.bridge_healthy)
$bridgePidDetected = [bool]($wslProbe -and $null -ne $wslProbe.runtime.bridge_pid)
$bridgeServiceActive = [bool]($wslProbe -and $wslProbe.runtime.bridge_service_active)
$claudeDetected = [bool]($wslProbe -and ($null -ne $wslProbe.runtime.claude_pid -or $wslProbe.runtime.port_8765 -or $wslProbe.runtime.port_8766))
$unitMatchesProject = [bool]($wslProbe -and ($wslProbe.runtime.unit_matches_project -eq $true -or $null -eq $wslProbe.runtime.unit_matches_project))

$overall = "not_ready"
if ($bridgeHealthy -and $claudeDetected -and $unitMatchesProject) {
  $overall = "ready"
} elseif ($bridgeHealthy) {
  $overall = "bridge_ready"
} elseif ($bridgePidDetected -or $bridgeServiceActive) {
  $overall = "degraded"
}

$bridgeEvidence = "none"
if ($bridgeHealthy) {
  $bridgeEvidence = "health"
} elseif ($bridgePidDetected) {
  $bridgeEvidence = "pid"
} elseif ($bridgeServiceActive) {
  $bridgeEvidence = "systemd"
}

$report = [ordered]@{
  schema_version = 1
  generated_at = (Get-Date).ToUniversalTime().ToString("o")
  overall = $overall
  bridge_evidence = $bridgeEvidence
  bridge_healthy = $bridgeHealthy
  bridge_pid_detected = $bridgePidDetected
  bridge_service_active = $bridgeServiceActive
  claude_detected = $claudeDetected
  unit_matches_project = $unitMatchesProject
  project_root = $ProjectRoot
  project_wsl = $projectWsl
  wsl = [ordered]@{
    distro = $Distro
    user = $User
    distros = $distros
  }
  runtime = if ($wslProbe) { $wslProbe.runtime } else { $null }
  components = if ($wslProbe) { $wslProbe.components } else { $null }
  windows_ports = if ($IncludeWindowsPorts) { Get-WindowsListeners -Ports @($ProxyPort, 8765, 8766, 9877, 443) } else { $null }
  warnings = @($warnings)
  secrets = [ordered]@{
    values_included = $false
  }
}

$report | ConvertTo-Json -Depth 10
