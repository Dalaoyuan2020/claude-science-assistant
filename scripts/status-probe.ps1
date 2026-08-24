[CmdletBinding()]
param(
  [string]$ProjectRoot = "",
  [string]$Distro = "",
  [string]$User = "",
  [int]$ProxyPort = 9876,
  [switch]$IncludeWindowsPorts,
  [switch]$DeepNetworkProbe
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

function Get-OptionalProperty {
  param($InputObject, [string]$Name, $Fallback = $null)
  if ($null -eq $InputObject) { return $Fallback }
  $property = $InputObject.PSObject.Properties[$Name]
  if ($null -eq $property) { return $Fallback }
  return $property.Value
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
  $previousErrorAction = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $raw = @(& wsl.exe @wslArgs @Command 2>$null)
    $exitCode = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $previousErrorAction
  }
  $lines = @(
    $raw |
      ForEach-Object { ("$_" -replace [char]0, "").TrimEnd() } |
      Where-Object { $_ }
  )
  if ($exitCode -ne 0) {
    $message = ($lines -join "`n").Trim()
    if (-not $message) { $message = "wsl.exe exited with code $exitCode (stderr suppressed to remove the known localhost-proxy/NAT warning)." }
    throw $message
  }
  return $lines
}

function Normalize-LocalPath {
  param([string]$Path)
  if (-not $Path) { return "" }
  $expanded = [Environment]::ExpandEnvironmentVariables($Path)
  if ($expanded.StartsWith('\\?\')) { return $expanded.Substring(4) }
  return $expanded
}

function Get-DriveSnapshot {
  param([string]$Path)
  $clean = Normalize-LocalPath $Path
  if (-not $clean) { return $null }
  $root = [IO.Path]::GetPathRoot($clean)
  if (-not $root) { return $null }
  try {
    $drive = [IO.DriveInfo]::new($root)
    return [ordered]@{
      drive = $drive.Name.TrimEnd('\')
      free_gb = [math]::Round($drive.AvailableFreeSpace / 1GB, 1)
      total_gb = [math]::Round($drive.TotalSize / 1GB, 1)
    }
  } catch { return $null }
}

function Get-WslHostStorage {
  param([string]$Distribution)
  if (-not $Distribution) { return $null }
  $entry = Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss' -ErrorAction SilentlyContinue |
    ForEach-Object { Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue } |
    Where-Object { ($_.PSObject.Properties.Name -contains 'DistributionName') -and $_.DistributionName -eq $Distribution } |
    Select-Object -First 1
  if (-not $entry) { return $null }
  $base = Normalize-LocalPath ([string]$entry.BasePath)
  $drive = Get-DriveSnapshot $base
  $vhdx = if ($base) { Join-Path $base 'ext4.vhdx' } else { "" }
  $vhdxItem = if ($vhdx -and (Test-Path -LiteralPath $vhdx)) { Get-Item -LiteralPath $vhdx -ErrorAction SilentlyContinue } else { $null }
  return [ordered]@{
    base_path = $base
    drive = if ($drive) { $drive.drive } else { $null }
    free_gb = if ($drive) { $drive.free_gb } else { $null }
    total_gb = if ($drive) { $drive.total_gb } else { $null }
    vhdx_size_gb = if ($vhdxItem) { [math]::Round($vhdxItem.Length / 1GB, 1) } else { $null }
  }
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
            command_line_included = $false
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
$hostStorage = Get-WslHostStorage -Distribution $Distro
$settingsStorage = Get-DriveSnapshot $env:APPDATA

$projectExists = (Test-Path -LiteralPath (Join-Path $ProjectRoot "proxy.py")) -and
  (Test-Path -LiteralPath (Join-Path $ProjectRoot "scripts\start-claude-science-wsl.sh")) -and
  (Test-Path -LiteralPath (Join-Path $ProjectRoot "scripts\csa-runtime-layout.sh")) -and
  (Test-Path -LiteralPath (Join-Path $ProjectRoot "scripts\csa-network-quality.py"))

$projectWsl = ""
$wslProbe = $null
$probeError = ""
if (-not $distros.Count -or -not $Distro) {
  $warnings.Add("No usable WSL distro was detected.")
} elseif (-not $projectExists) {
  $warnings.Add("ProjectRoot is not a complete CSA package root.")
} else {
  if (-not $User) {
    $User = (@(Invoke-Wsl @("id", "-un")) | Select-Object -First 1).Trim()
  }
  try {
    $projectWsl = Convert-ToWslPath $ProjectRoot
    $inspectWsl = "$projectWsl/skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh"
    $probeEnvironment = @("env", "PROJECT_DIR=$projectWsl", "PROXY_PORT=$ProxyPort")
    if ($DeepNetworkProbe) {
      $probeEnvironment += @("CSA_DEEP_NETWORK_PROBE=1", "CSA_WRITE_NETWORK_CACHE=1")
    }
    $rawProbe = ((Invoke-Wsl @($probeEnvironment + @("bash", $inspectWsl, $projectWsl))) -replace [char]0, "")
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
if ($wslProbe -and $wslProbe.runtime.bridge_health_responding -and $wslProbe.runtime.bridge_source_matches -eq $false) {
  $warnings.Add("Port 9876 is answered by a Bridge from another or older CSA package directory; start the current package once to migrate it.")
}
if ($wslProbe -and $null -ne $wslProbe.components.tmp_writable -and -not $wslProbe.components.tmp_writable) {
  $warnings.Add("WSL /tmp is not writable; Claude Science cannot start its sandbox/runtime probe. Run wsl --shutdown and reopen Ubuntu, or repair/recreate the WSL distro if it remains read-only.")
}
if ($wslProbe -and $null -ne $wslProbe.components.home_writable -and -not $wslProbe.components.home_writable) {
  $warnings.Add("WSL user home is not writable; CSA cannot update logs, systemd user units, or Bridge config in ~/.claude-science.")
}
if ($probeError) {
  $warnings.Add("WSL status probe failed: $probeError")
}

$rootFreeGb = $null
$rootFreeRatio = $null
$inodeFreeRatio = $null
$rootReadOnly = $false
if ($wslProbe -and $wslProbe.storage) {
  if ($null -ne $wslProbe.storage.root_free_kb) {
    $rootFreeGb = [math]::Round([double]$wslProbe.storage.root_free_kb / 1MB, 1)
  }
  if ($wslProbe.storage.root_total_kb -and $null -ne $wslProbe.storage.root_free_kb) {
    $rootFreeRatio = [double]$wslProbe.storage.root_free_kb / [double]$wslProbe.storage.root_total_kb
  }
  if ($wslProbe.storage.root_inode_total -and $null -ne $wslProbe.storage.root_inode_free) {
    $inodeFreeRatio = [double]$wslProbe.storage.root_inode_free / [double]$wslProbe.storage.root_inode_total
  }
  $rootReadOnly = [bool]$wslProbe.storage.root_read_only
}
$hostFreeGb = if ($hostStorage) { $hostStorage.free_gb } else { $null }
$settingsFreeGb = if ($settingsStorage) { $settingsStorage.free_gb } else { $null }
$tmpOrHomeBlocked = [bool]($wslProbe -and ((-not $wslProbe.components.tmp_writable) -or (-not $wslProbe.components.home_writable)))
$storageBlocked = [bool](
  $rootReadOnly -or
  $tmpOrHomeBlocked -or
  ($null -ne $hostFreeGb -and $hostFreeGb -lt 1) -or
  ($null -ne $rootFreeGb -and $rootFreeGb -lt 1) -or
  ($null -ne $settingsFreeGb -and $settingsFreeGb -lt 1) -or
  ($null -ne $rootFreeRatio -and $rootFreeRatio -lt 0.01) -or
  ($null -ne $inodeFreeRatio -and $inodeFreeRatio -lt 0.01)
)
$storageWarning = [bool](
  $storageBlocked -or
  ($null -ne $hostFreeGb -and $hostFreeGb -lt 15) -or
  ($null -ne $rootFreeGb -and $rootFreeGb -lt 15) -or
  ($null -ne $settingsFreeGb -and $settingsFreeGb -lt 10) -or
  ($null -ne $rootFreeRatio -and $rootFreeRatio -lt 0.10) -or
  ($null -ne $inodeFreeRatio -and $inodeFreeRatio -lt 0.05)
)
if ($rootReadOnly) {
  $warnings.Add("WSL root filesystem is mounted read-only; do not run automatic repair/restart until the host disk and VHDX are healthy.")
}
if ($null -ne $hostFreeGb -and $hostFreeGb -lt 15) {
  $warnings.Add("The WSL VHDX host volume has only $hostFreeGb GB free at $($hostStorage.base_path).")
}
if ($null -ne $rootFreeGb -and $rootFreeGb -lt 15) {
  $warnings.Add("The WSL Linux root filesystem has only $rootFreeGb GB free.")
}
if ($null -ne $settingsFreeGb -and $settingsFreeGb -lt 10) {
  $warnings.Add("The Windows settings drive $($settingsStorage.drive) has only $settingsFreeGb GB free; API Key switching may fail if it fills up.")
}
if ($null -ne $inodeFreeRatio -and $inodeFreeRatio -lt 0.05) {
  $warnings.Add("The WSL Linux root filesystem is low on free inodes.")
}
if ($wslProbe -and $wslProbe.storage.bridge_log_bytes -gt 52428800) {
  $warnings.Add("The Bridge log exceeds 50 MB and will be rotated on the next Bridge restart.")
}

$bridgeHealthy = [bool]($wslProbe -and $wslProbe.runtime.bridge_healthy)
$bridgePidDetected = [bool]($wslProbe -and $null -ne $wslProbe.runtime.bridge_pid)
$bridgeServiceActive = [bool]($wslProbe -and $wslProbe.runtime.bridge_service_active)
$claudeDetected = [bool]($wslProbe -and $wslProbe.runtime.claude_owner_verified -and $null -ne $wslProbe.runtime.claude_pid -and $wslProbe.runtime.port_8765 -and $wslProbe.runtime.port_8766)
if ($wslProbe -and ($wslProbe.runtime.port_8765 -or $wslProbe.runtime.port_8766) -and -not $wslProbe.runtime.claude_owner_verified) {
  $warnings.Add("Claude Science port 8765/8766 has an unverified owner; CSA will not treat it as running or stop it automatically.")
}
$systemdRunning = [bool]($wslProbe -and $wslProbe.wsl.systemd)
$unitMatchesProject = [bool]($wslProbe -and ((-not $systemdRunning) -or $wslProbe.runtime.unit_matches_project -eq $true))
$proxyState = if ($wslProbe -and $wslProbe.network) { [string]$wslProbe.network.proxy_state } else { "unknown" }
$proxyContractReady = $proxyState -in @("direct", "reachable")
$networkProbe = if ($wslProbe) { $wslProbe.network } else { $null }
$sandboxPairCount = [int](Get-OptionalProperty $networkProbe "sandbox_forwarder_count" 0)
$sandboxExpectedCount = [int](Get-OptionalProperty $networkProbe "sandbox_forwarder_expected_count" 0)
$sandboxHttpCount = [int](Get-OptionalProperty $networkProbe "sandbox_http_forwarder_count" 0)
$sandboxSocksCount = [int](Get-OptionalProperty $networkProbe "sandbox_socks_forwarder_count" 0)
$sandboxTopologyState = [string](Get-OptionalProperty $networkProbe "sandbox_forwarder_topology_state" "incomplete")
$sandboxProbeIdentity = [string](Get-OptionalProperty $networkProbe "sandbox_probe_identity" "")
$sandboxProbeRole = [string](Get-OptionalProperty $networkProbe "sandbox_probe_role" "")
$sandboxProbeTransport = [string](Get-OptionalProperty $networkProbe "sandbox_probe_transport" "")
$claudeProcessState = [string](Get-OptionalProperty $networkProbe "claude_process_state" "unknown")
$claudeWaitChannel = [string](Get-OptionalProperty $networkProbe "claude_wait_channel" "unknown")
$claudeIoBlocked = [bool](Get-OptionalProperty $networkProbe "claude_io_blocked" $false)
$claudeMountIoBlocked = [bool](Get-OptionalProperty $networkProbe "claude_mount_io_blocked" $false)
$sandboxUnixSocketState = [string](Get-OptionalProperty $networkProbe "sandbox_unix_socket_state" "not_checked")
$sandboxSocksHandshakeState = [string](Get-OptionalProperty $networkProbe "sandbox_socks_handshake_state" "not_checked")
$sandboxEgressFailureStage = [string](Get-OptionalProperty $networkProbe "sandbox_egress_failure_stage" "not_checked")
$sandboxProbeDaemonWaitChannel = [string](Get-OptionalProperty $networkProbe "sandbox_probe_daemon_wait_channel" "unknown")
$sandboxProbeDaemonIoBlocked = [bool](Get-OptionalProperty $networkProbe "sandbox_probe_daemon_io_blocked" $false)
$sandboxProbeDaemonMountIoBlocked = [bool](Get-OptionalProperty $networkProbe "sandbox_probe_daemon_mount_io_blocked" $false)
$sandboxEgressTarget = [string](Get-OptionalProperty $networkProbe "sandbox_egress_target" "")
$sandboxCanaryIdentity = [string](Get-OptionalProperty $networkProbe "sandbox_egress_canary_identity" "")
$sandboxDeepChecked = [bool](Get-OptionalProperty $networkProbe "deep_checked" $false)
$sandboxEgressState = [string](Get-OptionalProperty $networkProbe "sandbox_egress_state" "not_checked")
$sandboxContractStable = [bool](Get-OptionalProperty $networkProbe "sandbox_contract_stable_during_probe" $false)
$sandboxProbeCount = [int](Get-OptionalProperty $networkProbe "sandbox_forwarder_probe_count" 0)
$sandboxProbePassedCount = [int](Get-OptionalProperty $networkProbe "sandbox_forwarder_passed_count" 0)
$sandboxProbeFailedCount = [int](Get-OptionalProperty $networkProbe "sandbox_forwarder_failed_count" 0)
$sandboxEgressHttpStatus = [int](Get-OptionalProperty $networkProbe "sandbox_egress_http_status" 0)
$sandboxProbeContractReady = [bool](
  $sandboxProbeIdentity -eq "analysis-socks5h-pypi-head-v2" -and
  $sandboxProbeRole -eq "analysis" -and
  $sandboxProbeTransport -eq "socks5h" -and
  $sandboxEgressTarget -eq "pypi.org" -and
  $sandboxCanaryIdentity -eq "https://pypi.org/simple/pip/"
)
$sandboxForwardersReady = [bool](
  $wslProbe -and
  $sandboxExpectedCount -ge 3 -and
  $sandboxPairCount -ge $sandboxExpectedCount -and
  $sandboxHttpCount -ge $sandboxExpectedCount -and
  $sandboxSocksCount -ge $sandboxExpectedCount -and
  $sandboxProbeContractReady -and
  $sandboxTopologyState -in @("expected", "extended")
)
$deepEgressReady = [bool](
  $wslProbe -and
  $sandboxDeepChecked -and
  $sandboxContractStable -and
  $sandboxUnixSocketState -eq "connected" -and
  $sandboxSocksHandshakeState -eq "ok" -and
  $sandboxEgressState -eq "ok" -and
  $sandboxEgressFailureStage -eq "none" -and
  $sandboxProbeCount -eq 1 -and
  $sandboxProbePassedCount -eq 1 -and
  $sandboxProbeFailedCount -eq 0 -and
  $sandboxEgressHttpStatus -ge 200 -and
  $sandboxEgressHttpStatus -lt 300
)
$localNetworkReady = [bool](
  $claudeDetected -and
  -not $claudeIoBlocked -and
  $proxyContractReady -and
  $sandboxForwardersReady
)
$networkReady = [bool]($localNetworkReady -and $deepEgressReady)
$hostAccess = if ($wslProbe) { Get-OptionalProperty $wslProbe "host_access" $null } else { $null }
$preferencesPresent = [bool](Get-OptionalProperty $hostAccess "preferences_present" $false)
$preferencesParseOk = [bool](Get-OptionalProperty $hostAccess "preferences_parse_ok" $false)
$drvfsGrantCount = [int](Get-OptionalProperty $hostAccess "drvfs_write_grant_count" 0)
$broadGrantCount = [int](Get-OptionalProperty $hostAccess "broad_drvfs_write_grant_count" 0)

if ($claudeDetected -and $proxyState -in @("unreachable", "conflict", "invalid", "unknown")) {
  $endpoints = @($wslProbe.network.proxy_endpoints) -join ", "
  if (-not $endpoints) { $endpoints = "unknown" }
  $warnings.Add("Claude Science outbound proxy contract is $proxyState ($endpoints). Local ports can remain open while sandbox/API requests fail.")
}
if ($claudeDetected -and -not $sandboxForwardersReady) {
  $warnings.Add("Claude Science is listening, but its owned sandbox HTTP/SOCKS pair topology is not ready ($sandboxPairCount/$sandboxExpectedCount, state=$sandboxTopologyState).")
}
if ($wslProbe -and $sandboxDeepChecked -and -not $deepEgressReady) {
  if ($sandboxProbeDaemonMountIoBlocked -or $sandboxEgressState -eq "daemon_mount_io_busy") {
    if (-not $claudeIoBlocked) {
      $warnings.Add("The deep quality check observed transient WSL mount I/O, but the live Claude Science daemon has recovered to $claudeProcessState/$claudeWaitChannel. The transient result was not retained and does not prevent opening the local UI.")
    }
  } elseif ($sandboxProbeDaemonIoBlocked -or $sandboxEgressState -eq "daemon_busy") {
    if (-not $claudeIoBlocked) {
      $warnings.Add("The deep quality check observed transient daemon I/O, but the live Claude Science daemon has recovered. The transient result was not retained and does not prevent opening the local UI.")
    }
  } else {
    $warnings.Add("Sandbox egress quality check failed at $sandboxEgressFailureStage`: $sandboxEgressState. This does not prevent opening the verified local UI; no billable model request was made.")
  }
}
if ($claudeDetected -and $claudeMountIoBlocked) {
  if ($preferencesPresent -and -not $preferencesParseOk) {
    $warnings.Add("Claude Science is currently in scheduler state $claudeProcessState at $claudeWaitChannel. Persistent host-access scope could not be verified, so CSA will not recommend an authorization change; do not force a partial restart until the I/O returns.")
  } elseif ($broadGrantCount -gt 0) {
    $warnings.Add("Claude Science is currently in scheduler state $claudeProcessState at $claudeWaitChannel, and CSA detected $broadGrantCount broad persistent writable Windows grant(s). Do not force a partial restart until the I/O returns; then convert the broad grant to read-only or move the hot repository to WSL ext4.")
  } else {
    $warnings.Add("Claude Science is currently in scheduler state $claudeProcessState at $claudeWaitChannel. No broad persistent writable Windows grant was detected; this may be transient activity from an open Windows-backed workspace or concurrent MCP work. Do not force a partial restart until the I/O returns.")
  }
} elseif ($claudeDetected -and $claudeIoBlocked) {
  $warnings.Add("Claude Science is currently in uninterruptible I/O ($claudeProcessState at $claudeWaitChannel); CSA will not report network ready or attempt a partial restart until it becomes safely stoppable.")
}
$hostAccessRepairNeeded = $false
if ($hostAccess) {
  if ($preferencesPresent -and -not $preferencesParseOk) {
    $warnings.Add("Claude Science preferences.json exists but its host-access grants could not be parsed safely; no granted path was walked or modified.")
  }
  if ($drvfsGrantCount -gt 0) {
    $hostAccessRepairNeeded = $preferencesPresent -and $preferencesParseOk
    $drvfsGrants = @((Get-OptionalProperty $hostAccess "drvfs_write_grants" @())) -join ", "
    if (-not $drvfsGrants) { $drvfsGrants = "path details unavailable" }
    $warnings.Add("Detected $drvfsGrantCount persistent writable Windows/DrvFS grant(s) ($broadGrantCount standard broad-root match): $drvfsGrants. Repair and Restart preserves read access, stores a private backup, and converts these grants to read-only; keep writable work on WSL ext4 or grant only a narrow output leaf.")
  }
}

$overall = "not_ready"
if ($hostAccessRepairNeeded) {
  $overall = "repair_required"
} elseif ($bridgeHealthy -and $claudeDetected -and $unitMatchesProject -and $localNetworkReady) {
  $overall = if ($storageWarning) { "ready_with_storage_warning" } else { "ready" }
} elseif ($bridgeHealthy) {
  $overall = "bridge_ready"
} elseif ($bridgePidDetected -or $bridgeServiceActive) {
  $overall = "degraded"
} elseif ($storageBlocked) {
  $overall = "storage_blocked"
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
  schema_version = 2
  generated_at = (Get-Date).ToUniversalTime().ToString("o")
  overall = $overall
  bridge_evidence = $bridgeEvidence
  bridge_healthy = $bridgeHealthy
  bridge_pid_detected = $bridgePidDetected
  bridge_service_active = $bridgeServiceActive
  claude_detected = $claudeDetected
  unit_matches_project = $unitMatchesProject
  local_network_ready = $localNetworkReady
  network_ready = $networkReady
  project_root = $ProjectRoot
  project_wsl = $projectWsl
  wsl = [ordered]@{
    distro = $Distro
    user = $User
    distros = $distros
  }
  runtime = if ($wslProbe) { $wslProbe.runtime } else { $null }
  network = if ($wslProbe) { $wslProbe.network } else { $null }
  host_access = if ($wslProbe) { Get-OptionalProperty $wslProbe "host_access" $null } else { $null }
  components = if ($wslProbe) { $wslProbe.components } else { $null }
  storage = [ordered]@{
    blocked = $storageBlocked
    warning = $storageWarning
    wsl_host = $hostStorage
    windows_settings = $settingsStorage
    linux = if ($wslProbe) { $wslProbe.storage } else { $null }
  }
  windows_ports = if ($IncludeWindowsPorts) { Get-WindowsListeners -Ports @($ProxyPort, 8765, 8766, 9877, 443) } else { $null }
  warnings = @($warnings)
  secrets = [ordered]@{
    values_included = $false
  }
}

$report | ConvertTo-Json -Depth 10
