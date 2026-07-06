param(
  [string]$BridgeUrl = "",
  [string]$OutboundProxyUrl = "",
  [string]$ClaudeExe = "",
  [switch]$NoProxyEnv
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Resolve-Path (Join-Path $ScriptDir "..")
$ConfigPath = Join-Path $ProjectDir "config.json"

function Get-ConfigValue {
  param([string]$Name)
  if (-not (Test-Path $ConfigPath)) { return "" }
  try {
    $cfg = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    $value = $cfg.$Name
    if ($null -eq $value) { return "" }
    return [string]$value
  } catch {
    return ""
  }
}

function Get-BridgeUrl {
  if ($BridgeUrl) { return $BridgeUrl }
  $userBase = [Environment]::GetEnvironmentVariable("ANTHROPIC_BASE_URL", "User")
  if ($userBase) { return $userBase }
  $hostValue = Get-ConfigValue "proxy_host"
  $portValue = Get-ConfigValue "proxy_port"
  if (-not $hostValue) { $hostValue = "127.0.0.1" }
  if (-not $portValue) { $portValue = "9876" }
  $url = "http://${hostValue}:${portValue}"
  $token = (Get-ConfigValue "proxy_auth_token").Trim()
  $mode = (Get-ConfigValue "proxy_auth_mode").Trim().ToLowerInvariant()
  if ($token -and $mode -eq "required") {
    $url = "$url/$token"
  }
  return $url
}

function Get-OutboundProxyUrl {
  if ($OutboundProxyUrl) { return $OutboundProxyUrl }
  if ($env:CLAUDE_APP_PROXY_URL) { return $env:CLAUDE_APP_PROXY_URL }
  if ($env:OUTBOUND_PROXY_URL) { return $env:OUTBOUND_PROXY_URL }
  return Get-ConfigValue "outbound_proxy_url"
}

function Find-ClaudeExe {
  if ($ClaudeExe) {
    if (-not (Test-Path $ClaudeExe)) { throw "ClaudeExe was not found: $ClaudeExe" }
    return (Resolve-Path $ClaudeExe).Path
  }

  $stub = Join-Path $env:LOCALAPPDATA "AnthropicClaude\claude.exe"
  if (Test-Path $stub) { return (Resolve-Path $stub).Path }

  $root = Join-Path $env:LOCALAPPDATA "AnthropicClaude"
  if (Test-Path $root) {
    $latest = Get-ChildItem -LiteralPath $root -Directory -Filter "app-*" |
      Sort-Object LastWriteTime -Descending |
      Select-Object -First 1
    if ($latest) {
      $exe = Join-Path $latest.FullName "claude.exe"
      if (Test-Path $exe) { return (Resolve-Path $exe).Path }
    }
  }

  $cmd = Get-Command "claude.exe" -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }

  throw "Official Claude for Windows was not found. Install it from https://claude.com/download or with: winget install --id Anthropic.Claude"
}

function Add-NoProxyLocalhost {
  $existing = $env:NO_PROXY
  $required = @("127.0.0.1", "localhost")
  if (-not $existing) {
    $env:NO_PROXY = ($required -join ",")
    return
  }

  $parts = $existing.Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ }
  foreach ($item in $required) {
    if ($parts -notcontains $item) { $parts += $item }
  }
  $env:NO_PROXY = ($parts -join ",")
}

$ResolvedBridgeUrl = Get-BridgeUrl
$ResolvedProxyUrl = Get-OutboundProxyUrl
$ResolvedClaudeExe = Find-ClaudeExe

[Environment]::SetEnvironmentVariable("ANTHROPIC_BASE_URL", $ResolvedBridgeUrl, "User")
$env:ANTHROPIC_BASE_URL = $ResolvedBridgeUrl

if (-not $NoProxyEnv -and $ResolvedProxyUrl) {
  $env:HTTP_PROXY = $ResolvedProxyUrl
  $env:HTTPS_PROXY = $ResolvedProxyUrl
  Add-NoProxyLocalhost
}

Write-Host "Starting official Claude:"
Write-Host "  exe: $ResolvedClaudeExe"
Write-Host "  ANTHROPIC_BASE_URL: $($ResolvedBridgeUrl -replace '(://[^/]+/).+', '$1****')"
if (-not $NoProxyEnv -and $ResolvedProxyUrl) {
  Write-Host "  HTTP(S)_PROXY: $ResolvedProxyUrl"
  Write-Host "  NO_PROXY: $env:NO_PROXY"
}

Start-Process -FilePath $ResolvedClaudeExe -WorkingDirectory (Split-Path -Parent $ResolvedClaudeExe)
