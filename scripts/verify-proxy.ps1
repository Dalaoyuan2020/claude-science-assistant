param(
  [string]$Python = $(if ($env:PYTHON) { $env:PYTHON } else { ".\.venv\Scripts\python.exe" }),
  [switch]$VerifyImage
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Resolve-Path (Join-Path $ScriptDir "..")
Set-Location $ProjectDir

if (-not (Test-Path $Python)) {
  $cmd = Get-Command python -ErrorAction SilentlyContinue
  if (-not $cmd) { throw "Python not found. Run scripts/install-safe.ps1 first." }
  $Python = $cmd.Source
}

$ConfigPath = Join-Path $ProjectDir "config.json"
$BaseUrl = "http://127.0.0.1:9876"
if (Test-Path $ConfigPath) {
  $code = @'
import json
import sys
from pathlib import Path
cfg = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
host = str(cfg.get("proxy_host") or "127.0.0.1")
port = str(cfg.get("proxy_port") or "9876")
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
    $BaseUrl = (& $Python $tmp $ConfigPath).Trim()
  } finally {
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
  }
}

$DisplayBaseUrl = $BaseUrl -replace '(://[^/]+/).+', '$1****'
Write-Host "Verifying proxy at $DisplayBaseUrl"

Write-Host "1. health"
$health = Invoke-RestMethod -Uri "$BaseUrl/health" -TimeoutSec 5
if ($health.status -ne "ok") { throw "health did not return ok" }
if (-not ($health.deepseek_configured -or $health.openai_configured -or $health.custom_configured)) {
  throw "No backend API key is configured. Configure config.json or dashboard first."
}
$health | ConvertTo-Json -Depth 6

Write-Host "2. models"
$models = Invoke-RestMethod -Uri "$BaseUrl/v1/models" -TimeoutSec 5
if (-not $models.data -or $models.data.Count -lt 1) { throw "models endpoint returned no models" }
Write-Host "models=$($models.data.Count)"

Write-Host "3. messages"
$body = @{
  model = "claude-sonnet-4-5"
  max_tokens = 32
  messages = @(@{ role = "user"; content = "Reply with OK." })
} | ConvertTo-Json -Depth 8
$message = Invoke-RestMethod -Uri "$BaseUrl/v1/messages" -Method Post -ContentType "application/json" -Body $body -TimeoutSec 60
if ($message.type -eq "error" -or $message.error) { throw ($message | ConvertTo-Json -Depth 8) }
if ($message.type -ne "message") { throw "message endpoint did not return an Anthropic message" }
Write-Host "message_id=$($message.id) stop_reason=$($message.stop_reason)"

Write-Host "4. recent requests"
$recent = Invoke-RestMethod -Uri "$BaseUrl/api/recent-requests" -TimeoutSec 5
$success = @($recent.requests | Where-Object { $_.backend -in @("deepseek", "openai", "custom") -and $_.status -eq "success" })
if ($success.Count -lt 1) { throw "No successful backend request found in recent requests." }
Write-Host "successful_backend_requests=$($success.Count)"

if ($VerifyImage -or ($env:VERIFY_IMAGE -eq "1")) {
  Write-Host "5. image message"
  Add-Type -AssemblyName System.Drawing
  $bitmap = New-Object System.Drawing.Bitmap 32, 32
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  $graphics.Clear([System.Drawing.Color]::Red)
  $stream = New-Object System.IO.MemoryStream
  $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
  $graphics.Dispose()
  $bitmap.Dispose()
  $image64 = [Convert]::ToBase64String($stream.ToArray())
  $stream.Dispose()
  $imageBody = @{
    model = "claude-opus-4-8"
    max_tokens = 32
    messages = @(@{
      role = "user"
      content = @(
        @{ type = "text"; text = "Look at the image. If the dominant color is red, reply exactly: red. Otherwise reply exactly: no." },
        @{ type = "image"; source = @{ type = "base64"; media_type = "image/png"; data = $image64 } }
      )
    })
  } | ConvertTo-Json -Depth 12
  $imageMessage = Invoke-RestMethod -Uri "$BaseUrl/v1/messages" -Method Post -ContentType "application/json" -Body $imageBody -TimeoutSec 90
  $text = (($imageMessage.content | Where-Object { $_.type -eq "text" } | ForEach-Object { $_.text }) -join " ")
  if ($text -notmatch "\bred\b") { throw "Image verification did not confirm red. Response: $text" }
  Write-Host "image_response=$text"
}

Write-Host "proxy verification passed"
