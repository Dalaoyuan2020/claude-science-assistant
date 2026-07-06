param(
  [string]$ProxyHost = $(if ($env:PROXY_HOST) { $env:PROXY_HOST } else { "127.0.0.1" }),
  [int]$ProxyPort = $(if ($env:PROXY_PORT) { [int]$env:PROXY_PORT } else { 9876 }),
  [switch]$NoStart
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Resolve-Path (Join-Path $ScriptDir "..")
$TaskName = "ClaudeScienceByokProxy"
$StateDir = Join-Path $HOME ".claude-science"
$LogDir = Join-Path $StateDir "logs"
$ConfigPath = Join-Path $ProjectDir "config.json"
$ConfigExamplePath = Join-Path $ProjectDir "config.example.json"

function Find-Python {
  if ($env:PYTHON -and (Test-Path $env:PYTHON)) {
    return (Resolve-Path $env:PYTHON).Path
  }
  $candidates = @("py", "python")
  foreach ($candidate in $candidates) {
    $cmd = Get-Command $candidate -ErrorAction SilentlyContinue
    if (-not $cmd) { continue }
    if ($candidate -eq "py") {
      $version = & $cmd.Source -3 --version 2>$null
      if ($LASTEXITCODE -eq 0) { return "$($cmd.Source) -3" }
    } else {
      $version = & $cmd.Source --version 2>$null
      if ($LASTEXITCODE -eq 0) { return $cmd.Source }
    }
  }
  throw "Python 3 was not found. Install Python 3, or set PYTHON to python.exe."
}

function Invoke-Python {
  param([string]$PythonCommand, [string[]]$Arguments)
  if ($PythonCommand -like "* -3") {
    $exe = $PythonCommand.Substring(0, $PythonCommand.Length - 3)
    & $exe -3 @Arguments
  } else {
    & $PythonCommand @Arguments
  }
}

function Get-ProxyUrl {
  param([string]$PythonExe)
  $code = @'
import json
import os
import sys
from pathlib import Path

config_path = Path(sys.argv[1])
host = sys.argv[2]
port = sys.argv[3]
data = json.loads(config_path.read_text(encoding="utf-8"))
host = str(data.get("proxy_host") or host)
port = str(data.get("proxy_port") or port)
url = f"http://{host}:{port}"
token = str(data.get("proxy_auth_token") or "").strip()
mode = str(data.get("proxy_auth_mode") or "optional").lower()
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

function Write-Runner {
  param([string]$PythonExe, [string]$ProxyUrl)
  $runner = Join-Path $StateDir "run-proxy.ps1"
  $proxyPy = Join-Path $ProjectDir "proxy.py"
  $logFile = Join-Path $LogDir "proxy.log"
  $content = @"
`$ErrorActionPreference = "Stop"
`$env:ANTHROPIC_BASE_URL = "$ProxyUrl"
`$env:PROXY_HOST = "$ProxyHost"
`$env:PROXY_PORT = "$ProxyPort"
Set-Location "$ProjectDir"
& "$PythonExe" "$proxyPy" *>> "$logFile"
"@
  Set-Content -LiteralPath $runner -Value $content -Encoding UTF8
  return $runner
}

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

$BootstrapPython = Find-Python
Write-Host "Using bootstrap Python: $BootstrapPython"

$VenvDir = if ($env:VENV_DIR) { $env:VENV_DIR } else { Join-Path $ProjectDir ".venv" }
$VenvPython = Join-Path $VenvDir "Scripts\python.exe"
if (-not (Test-Path $VenvPython)) {
  Invoke-Python $BootstrapPython @("-m", "venv", $VenvDir)
}
$PythonBin = (Resolve-Path $VenvPython).Path
Write-Host "Using runtime Python: $PythonBin"

& $PythonBin -m pip install --upgrade pip
& $PythonBin -m pip install -r (Join-Path $ProjectDir "requirements.txt")

if (-not (Test-Path $ConfigPath)) {
  Copy-Item -LiteralPath $ConfigExamplePath -Destination $ConfigPath
  Write-Host "Created config.json from config.example.json"
}

$envCode = @'
import json
import os
import sys
from pathlib import Path

path = Path(sys.argv[1])
data = json.loads(path.read_text(encoding="utf-8"))
scalar = {
    "DEEPSEEK_API_KEY": "deepseek_api_key",
    "OPENAI_API_KEY": "openai_api_key",
    "CUSTOM_API_KEY": "custom_api_key",
    "DEEPSEEK_BASE_URL": "deepseek_base_url",
    "OPENAI_BASE_URL": "openai_base_url",
    "CUSTOM_BASE_URL": "custom_base_url",
    "DEFAULT_BACKEND": "default_backend",
    "FORCE_MODEL": "force_model",
    "MODEL_LIST_MODE": "model_list_mode",
    "DEFAULT_MAX_TOKENS_CAP": "default_max_tokens_cap",
    "DEEPSEEK_UPSTREAM_MODE": "deepseek_upstream_mode",
    "OPENAI_UPSTREAM_MODE": "openai_upstream_mode",
    "CUSTOM_UPSTREAM_MODE": "custom_upstream_mode",
    "PROXY_AUTH_TOKEN": "proxy_auth_token",
    "PROXY_AUTH_MODE": "proxy_auth_mode",
    "OUTBOUND_PROXY_URL": "outbound_proxy_url",
    "REASONING_CONTENT_POLICY": "reasoning_content_policy",
    "INLINE_IMAGE_POLICY": "inline_image_policy",
}
json_keys = {
    "DEEPSEEK_MODEL_MAP": "deepseek_model_map",
    "OPENAI_MODEL_MAP": "openai_model_map",
    "CUSTOM_MODEL_MAP": "custom_model_map",
    "MODEL_ALIASES": "model_aliases",
    "MODEL_TOKEN_CAPS": "model_token_caps",
}
changed = []
for env_key, cfg_key in scalar.items():
    value = os.environ.get(env_key)
    if value:
        data[cfg_key] = int(value) if cfg_key == "default_max_tokens_cap" else value
        changed.append(cfg_key)
for env_key, cfg_key in json_keys.items():
    value = os.environ.get(env_key)
    if value:
        data[cfg_key] = json.loads(value)
        changed.append(cfg_key)
if changed:
    path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    public = [x for x in changed if not x.endswith("_api_key") and x != "proxy_auth_token"]
    secrets = len(changed) - len(public)
    print(f"Applied config from environment: {', '.join(public) or '(only secrets)'}; secrets updated: {secrets}")
'@
$tmpEnv = New-TemporaryFile
try {
  Set-Content -LiteralPath $tmpEnv -Value $envCode -Encoding UTF8
  & $PythonBin $tmpEnv $ConfigPath
} finally {
  Remove-Item -LiteralPath $tmpEnv -Force -ErrorAction SilentlyContinue
}

$ProxyUrl = Get-ProxyUrl $PythonBin
[Environment]::SetEnvironmentVariable("ANTHROPIC_BASE_URL", $ProxyUrl, "User")
$env:ANTHROPIC_BASE_URL = $ProxyUrl

$EncryptionKey = Join-Path $StateDir "encryption.key"
if (Test-Path $EncryptionKey) {
  & $PythonBin (Join-Path $ProjectDir "setup-token.py")
} else {
  Write-Host "Warning: $EncryptionKey does not exist yet. Fake OAuth token was not generated."
}

$Runner = Write-Runner $PythonBin $ProxyUrl
$Action = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$Runner`""
$Trigger = New-ScheduledTaskTrigger -AtLogOn
$UseRunKey = $false
try {
  Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Description "Claude Science BYOK proxy" -Force | Out-Null
} catch {
  $UseRunKey = $true
  $runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
  $runValue = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$Runner`""
  New-Item -Path $runKey -Force | Out-Null
  Set-ItemProperty -Path $runKey -Name $TaskName -Value $runValue
  Write-Host "Scheduled task registration failed; installed HKCU Run fallback instead."
}

if (-not $NoStart) {
  if ($UseRunKey) {
    Start-Process -FilePath "powershell.exe" -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $Runner) -WindowStyle Hidden
  } else {
    Start-ScheduledTask -TaskName $TaskName
  }
  Start-Sleep -Seconds 2
  try {
    Invoke-RestMethod -Uri "http://${ProxyHost}:${ProxyPort}/health" -TimeoutSec 5 | ConvertTo-Json -Depth 6
  } catch {
    Write-Host "Proxy task was installed, but health check did not respond yet: $($_.Exception.Message)"
  }
}

Write-Host "Safe Windows install complete."
Write-Host "Dashboard: http://${ProxyHost}:${ProxyPort}/dashboard"
Write-Host "ANTHROPIC_BASE_URL has been set for the current user."
