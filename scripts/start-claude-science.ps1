param(
  [string]$ProxyHost = $(if ($env:PROXY_HOST) { $env:PROXY_HOST } else { "127.0.0.1" }),
  [int]$ProxyPort = $(if ($env:PROXY_PORT) { [int]$env:PROXY_PORT } else { 9876 })
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Resolve-Path (Join-Path $ScriptDir "..")
$TaskName = "ClaudeScienceByokProxy"
$ConfigPath = Join-Path $ProjectDir "config.json"

function Find-Python {
  if ($env:PYTHON -and (Test-Path $env:PYTHON)) { return (Resolve-Path $env:PYTHON).Path }
  $venv = Join-Path $ProjectDir ".venv\Scripts\python.exe"
  if (Test-Path $venv) { return (Resolve-Path $venv).Path }
  $cmd = Get-Command python -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  throw "Python was not found. Run scripts/install-safe.ps1 first."
}

function Get-ProxyUrl {
  param([string]$PythonExe)
  if (-not (Test-Path $ConfigPath)) { return "http://${ProxyHost}:${ProxyPort}" }
  $code = @'
import json
import sys
from pathlib import Path
cfg = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
host = str(cfg.get("proxy_host") or sys.argv[2])
port = str(cfg.get("proxy_port") or sys.argv[3])
url = f"http://{host}:{port}"
token = str(cfg.get("proxy_auth_token") or "").strip()
mode = str(cfg.get("proxy_auth_mode") or "optional").lower()
if token and mode == "required":
    url += "/" + token
print(url)
'@
  $tmp = New-TemporaryFile
  try {
    Set-Content -LiteralPath $tmp -Value $code -Encoding UTF8
    return (& $PythonExe $tmp $ConfigPath $ProxyHost $ProxyPort).Trim()
  } finally {
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
  }
}

$PythonBin = Find-Python
$ProxyUrl = Get-ProxyUrl $PythonBin
[Environment]::SetEnvironmentVariable("ANTHROPIC_BASE_URL", $ProxyUrl, "User")
$env:ANTHROPIC_BASE_URL = $ProxyUrl

try {
  Start-ScheduledTask -TaskName $TaskName -ErrorAction Stop
} catch {
  Start-Process -FilePath $PythonBin -ArgumentList @((Join-Path $ProjectDir "proxy.py")) -WorkingDirectory $ProjectDir -WindowStyle Hidden
}

Write-Host "Proxy started or requested at http://${ProxyHost}:${ProxyPort}"
Write-Host "ANTHROPIC_BASE_URL=$($ProxyUrl -replace '(://[^/]+/).+', '$1****')"
