[CmdletBinding()]
param(
  [switch]$LiveConnectRuntime,
  [switch]$VerifyProxy,
  [string]$Distro = "Ubuntu-24.04",
  [string]$EvidenceDir = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$PackageRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
if (-not $EvidenceDir) { $EvidenceDir = Join-Path $PackageRoot "reports\package-acceptance" }
$EvidenceDir = (New-Item -ItemType Directory -Force -Path $EvidenceDir).FullName
$Cases = New-Object System.Collections.Generic.List[object]

function Invoke-Case {
  param(
    [Parameter(Mandatory = $true)][string]$Id,
    [Parameter(Mandatory = $true)][string]$Description,
    [Parameter(Mandatory = $true)][scriptblock]$Action
  )
  $timer = [Diagnostics.Stopwatch]::StartNew()
  try {
    & $Action
    $timer.Stop()
    $Cases.Add([ordered]@{ id = $Id; description = $Description; status = "passed"; durationMs = $timer.ElapsedMilliseconds })
    Write-Host "PASS $Id - $Description"
  } catch {
    $timer.Stop()
    $Cases.Add([ordered]@{ id = $Id; description = $Description; status = "failed"; durationMs = $timer.ElapsedMilliseconds; error = [string]$_.Exception.Message })
    Write-Host "FAIL $Id - $Description"
    throw
  }
}

$manifestPath = Join-Path $PackageRoot "manifest.json"
$manifest = $null
Invoke-Case "PKG-01" "Manifest and v0.2 feature inventory are complete" {
  $script:manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
  if ([string]$manifest.version -ne "0.2.0") { throw "Expected package version 0.2.0." }
  foreach ($path in @(
    "claude-science-assistant.exe",
    "proxy.py",
    "vendor\claude-science\linux-x64\claude-science",
    "vendor\csa-connect\linux-x64\csa-connect",
    "skills\csa-connect\SKILL.md",
    "skills\csa-external-agent\SKILL.md",
    "skills\csa-external-agent\scripts\submit-request.sh",
    "extensions\csa-claude-science-connector\manifest.json",
    "scripts\create-subagent-request.ps1",
    "docs\v0.2-new-features-acceptance.zh-CN.md",
    "docs\v0.2-install-upgrade-release-guide.zh-CN.md"
  )) {
    if (-not (Test-Path -LiteralPath (Join-Path $PackageRoot $path))) { throw "Package file is missing: $path" }
  }
}

Invoke-Case "PKG-02" "Bundled Linux binaries match their manifests" {
  foreach ($component in @("claude-science", "csa-connect")) {
    $dir = Join-Path $PackageRoot "vendor\$component\linux-x64"
    $info = Get-Content -LiteralPath (Join-Path $dir "manifest.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    $binaryName = if ($component -eq "csa-connect") { "csa-connect" } else { "claude-science" }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $dir $binaryName)).Hash.ToLowerInvariant()
    if ($actual -ne [string]$info.sha256) { throw "$component SHA-256 does not match its manifest." }
  }
}

Invoke-Case "PKG-03" "Subagent package entry enforces manual approval and credential rejection" {
  $fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ("csa-package-subagent-" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null
  try {
    & (Join-Path $ScriptDir "create-subagent-request.ps1") `
      -ProjectRoot $fixtureRoot `
      -TaskKind environment `
      -Title "Package verification" `
      -Note "Read-only diagnosis with redacted errors." | Out-Null
    $requestFile = @(Get-ChildItem -LiteralPath (Join-Path $fixtureRoot "reports\csa-agent-inbox") -Filter "*.json" -File)
    if ($requestFile.Count -ne 1) { throw "Package request script did not write exactly one request." }
    $request = Get-Content -LiteralPath $requestFile[0].FullName -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([string]$request.approvalMode -ne "manual" -or [string]$request.policyId -ne "manual-only") {
      throw "Package request script bypassed manual approval."
    }
    $rejected = $false
    try {
      & (Join-Path $ScriptDir "create-subagent-request.ps1") -ProjectRoot $fixtureRoot -Note "TOKEN=not-a-real-value" | Out-Null
    } catch { $rejected = $true }
    if (-not $rejected) { throw "Package request script accepted credential-like content." }
  } finally {
    Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}

Invoke-Case "PKG-04" "Packaged Bridge translation and safety tests pass" {
  & (Join-Path $ScriptDir "self-test.ps1")
  if ($LASTEXITCODE -ne 0) { throw "Packaged self-test failed with exit code $LASTEXITCODE." }
}

if ($LiveConnectRuntime) {
  Invoke-Case "PKG-LIVE-01" "Installed Connect Gateway exposes a ready local MCP queue" {
    $command = '$HOME/.local/share/claude-science-api-bridge/bin/csa-connect status --config $HOME/.local/share/claude-science-api-bridge/connect/config.json'
    $raw = & wsl.exe -d $Distro -- sh -lc $command
    if ($LASTEXITCODE -ne 0) { throw "Unable to read Connect Gateway status." }
    $status = ($raw -join "`n") | ConvertFrom-Json
    if (-not $status.running -or -not $status.mcpReady -or [string]$status.mcpUrl -ne "http://127.0.0.1:9881/mcp") {
      throw "Connect Gateway or MCP is not ready."
    }
  }
}

if ($VerifyProxy) {
  Invoke-Case "PKG-LIVE-02" "Installed Bridge passes live proxy verification" {
    & (Join-Path $ScriptDir "verify-proxy.ps1")
    if ($LASTEXITCODE -ne 0) { throw "verify-proxy.ps1 failed with exit code $LASTEXITCODE." }
  }
}

$failed = @($Cases | Where-Object { $_.status -ne "passed" }).Count
$evidence = [ordered]@{
  schemaVersion = 1
  generatedAt = (Get-Date).ToUniversalTime().ToString("o")
  packageVersion = [string]$manifest.version
  packageQualifier = [string]$manifest.packageQualifier
  sourceCommit = [string]$manifest.sourceCommit
  sourceTreeDirty = [bool]$manifest.sourceTreeDirty
  passed = $Cases.Count - $failed
  failed = $failed
  cases = $Cases.ToArray()
}
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$target = Join-Path $EvidenceDir "package-v0.2-$stamp.json"
$temporary = "$target.tmp"
$evidence | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $temporary -Encoding UTF8
Move-Item -LiteralPath $temporary -Destination $target -Force
Write-Host "Evidence: $target"
if ($failed -gt 0) { exit 1 }
Write-Host "Package verification passed"
