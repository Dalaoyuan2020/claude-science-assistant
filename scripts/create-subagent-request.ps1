[CmdletBinding()]
param(
  [string]$ProjectRoot = "",
  [ValidateSet("dataset", "environment", "vm", "migration", "custom")]
  [string]$TaskKind = "custom",
  [string]$Title = "Subagent request",
  [string]$Note = "Read-only diagnosis requested from sandbox.",
  [ValidateSet("diagnose", "plan", "review")]
  [string]$RequestedAction = "diagnose",
  [ValidateSet("manual", "autoCandidate")]
  [string]$ApprovalMode = "manual",
  [string]$PolicyId = "manual-only",
  [string]$Cwd = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $ProjectRoot) {
  $ProjectRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
} else {
  $ProjectRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
}
if (-not $Cwd) {
  $Cwd = (Get-Location).Path
}

if ($Title.Length -gt 240 -or $Note.Length -gt 12000 -or $Cwd.Length -gt 4096) {
  throw "Subagent request fields are too long. Shorten the title, note, or cwd."
}
if (($Title + $Note + $Cwd).Contains([char]0)) {
  throw "Subagent request fields must not contain NUL characters."
}
$SensitiveText = $Title + [Environment]::NewLine + $Note
$SensitivePatterns = @(
  '(?i)(api[_-]?key|(?:access[_-]?|refresh[_-]?|id[_-]?)?token|authorization|password|private[_-]?key|secret)\s*[:=]\s*\S+',
  '(?i)\bbearer\s+[A-Za-z0-9._~+/-]{8,}',
  '\bsk-[A-Za-z0-9_-]{12,}\b',
  '-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----'
)
foreach ($Pattern in $SensitivePatterns) {
  if ($SensitiveText -match $Pattern) {
    throw "Subagent request appears to contain a credential. Replace it with a redacted error summary."
  }
}

$Inbox = Join-Path $ProjectRoot "reports\csa-agent-inbox"
New-Item -ItemType Directory -Force -Path $Inbox | Out-Null

$RequestId = "req-" + (Get-Date -Format "yyyyMMdd-HHmmss") + "-" + ([guid]::NewGuid().ToString("N").Substring(0, 8))
$Path = Join-Path $Inbox "$RequestId.json"

$Request = [ordered]@{
  schemaVersion = 1
  source = "sandbox-cli"
  taskKind = $TaskKind
  title = $Title
  cwd = $Cwd
  note = $Note
  requestedAction = $RequestedAction
  approvalMode = $ApprovalMode
  policyId = $PolicyId
  createdAt = (Get-Date).ToUniversalTime().ToString("o")
}

$Json = $Request | ConvertTo-Json -Depth 6
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($Path, $Json + [Environment]::NewLine, $Utf8NoBom)
Write-Host "Subagent request written:"
Write-Host $Path
