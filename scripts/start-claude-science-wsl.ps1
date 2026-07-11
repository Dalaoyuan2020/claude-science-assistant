[CmdletBinding()]
param(
  [string]$Distro = "",
  [string]$User = "",
  [int]$ProxyPort = 9876,
  [int]$ClaudeSciencePort = 8765,
  [switch]$Open
)

$ErrorActionPreference = "Stop"

function Invoke-WslQuiet {
  param([string[]]$Arguments)
  $previousErrorAction = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $result = @(& wsl.exe @Arguments 2>$null)
    $exitCode = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $previousErrorAction
  }
  if ($exitCode -ne 0) {
    throw "wsl.exe failed with exit code $exitCode."
  }
  return $result
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

if (-not $Distro) {
  $Distro = Select-Distro -Distros @(Get-Distros)
}
if (-not $Distro) {
  throw "No usable WSL distro found. Ubuntu-24.04 is recommended, but CSA can use another installed Ubuntu/WSL distro."
}
if (-not $User) {
  $User = (((Invoke-WslQuiet @("-d", $Distro, "--", "id", "-un")) -replace [char]0, "") | Select-Object -First 1).Trim()
}
if (-not $User) {
  throw "Failed to detect WSL user for distro '$Distro'."
}

$ProjectDir = Resolve-Path (Join-Path $PSScriptRoot "..")
$ProjectPortablePath = $ProjectDir.Path.Replace("\", "/")
$ProjectWslOutput = @((Invoke-WslQuiet @("-d", $Distro, "-u", $User, "--", "wslpath", "-a", $ProjectPortablePath)) -replace [char]0, "")
$ProjectWsl = @(
  $ProjectWslOutput |
    ForEach-Object { "$_".Trim() } |
    Where-Object { $_ -match '^/' } |
    Select-Object -First 1
)
if (-not $ProjectWsl) {
  throw "Failed to convert project path to WSL path."
}
$ProjectWsl = [string]$ProjectWsl[0]

$previousErrorAction = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
  $output = @(& wsl.exe -d $Distro -u $User -- env `
    "PROXY_PORT=$ProxyPort" `
    "CLAUDE_SCIENCE_PORT=$ClaudeSciencePort" `
    "CSA_MERGE_STDERR=1" `
    bash "$ProjectWsl/scripts/start-claude-science-wsl.sh" 2>$null)
  $exitCode = $LASTEXITCODE
} finally {
  $ErrorActionPreference = $previousErrorAction
}

$output | ForEach-Object { Write-Host $_ }
if ($exitCode -ne 0) {
  throw "WSL start script failed with exit code $exitCode."
}

if ($Open) {
  $match = $output | Select-String -Pattern "http://localhost:\d+/\?nonce=[a-f0-9]+" | Select-Object -First 1
  if ($match) {
    Start-Process $match.Matches[0].Value
  } else {
    Write-Warning "Claude Science URL was not found in script output."
  }
}
