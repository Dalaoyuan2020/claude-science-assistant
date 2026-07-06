param(
  [int]$ProxyPort = $(if ($env:PROXY_PORT) { [int]$env:PROXY_PORT } else { 9876 })
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Resolve-Path (Join-Path $ScriptDir "..")
$ConfigPath = Join-Path $ProjectDir "config.json"

function Section($Name) {
  Write-Host ""
  Write-Host "== $Name =="
}

Section "System"
Get-ComputerInfo | Select-Object WindowsProductName, WindowsVersion, OsHardwareAbstractionLayer | Format-List
Write-Host "User: $env:USERNAME"
Write-Host "Project: $ProjectDir"

Section "Python"
foreach ($name in @("py", "python")) {
  $cmd = Get-Command $name -ErrorAction SilentlyContinue
  if ($cmd) {
    Write-Host "${name}: $($cmd.Source)"
    & $cmd.Source --version 2>$null
  }
}

Section "Environment"
Write-Host "ANTHROPIC_BASE_URL=$([Environment]::GetEnvironmentVariable('ANTHROPIC_BASE_URL', 'User'))"
Write-Host "OUTBOUND_PROXY_URL=$env:OUTBOUND_PROXY_URL"

Section "Ports"
foreach ($port in @($ProxyPort, 9877, 443, 8765)) {
  Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue |
    Select-Object LocalAddress, LocalPort, OwningProcess
}

Section "Scheduled Task"
Get-ScheduledTask -TaskName "ClaudeScienceByokProxy" -ErrorAction SilentlyContinue |
  Select-Object TaskName, State, TaskPath

Section "Files"
foreach ($path in @(
  (Join-Path $ProjectDir "proxy.py"),
  (Join-Path $ProjectDir "setup-token.py"),
  $ConfigPath,
  (Join-Path $ProjectDir "config.example.json"),
  (Join-Path $HOME ".claude-science\encryption.key")
)) {
  if (Test-Path $path) { Write-Host "ok   $path" } else { Write-Host "miss $path" }
}

Section "Config Summary"
if (Test-Path $ConfigPath) {
  $cfg = Get-Content -Raw -Encoding UTF8 $ConfigPath | ConvertFrom-Json
  Write-Host "default_backend=$($cfg.default_backend)"
  Write-Host "force_model=$($cfg.force_model)"
  Write-Host "model_list_mode=$($cfg.model_list_mode)"
  Write-Host "deepseek_upstream_mode=$($cfg.deepseek_upstream_mode)"
  Write-Host "openai_upstream_mode=$($cfg.openai_upstream_mode)"
  Write-Host "custom_upstream_mode=$($cfg.custom_upstream_mode)"
  Write-Host "proxy_auth_mode=$($cfg.proxy_auth_mode)"
  Write-Host "outbound_proxy_url=$($cfg.outbound_proxy_url)"
  Write-Host "deepseek_api_key=$(if ($cfg.deepseek_api_key) { 'yes' } else { 'no' })"
  Write-Host "openai_api_key=$(if ($cfg.openai_api_key) { 'yes' } else { 'no' })"
  Write-Host "custom_api_key=$(if ($cfg.custom_api_key) { 'yes' } else { 'no' })"
} else {
  Write-Host "config.json not found"
}

Section "HTTP Checks"
try {
  Invoke-RestMethod -Uri "http://127.0.0.1:$ProxyPort/health" -TimeoutSec 3 | ConvertTo-Json -Depth 6
} catch {
  Write-Host "health failed: $($_.Exception.Message)"
}
try {
  Invoke-RestMethod -Uri "http://127.0.0.1:$ProxyPort/api/recent-requests" -TimeoutSec 3 | ConvertTo-Json -Depth 4
} catch {
  Write-Host "recent requests failed: $($_.Exception.Message)"
}
