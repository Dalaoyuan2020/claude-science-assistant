[CmdletBinding()]
param(
  [string]$ProjectRoot = "",
  [string]$Distro = "",
  [string]$User = "",
  [switch]$ApproveInstall,
  [switch]$InstallWslIfMissing,
  [switch]$StartServices,
  [switch]$RunSelfTest,
  [switch]$SkipRollbackPreview
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $ProjectRoot) {
  $ProjectRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
} else {
  $ProjectRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
}

function Write-Step {
  param([string]$Message)
  Write-Host ""
  Write-Host "== $Message =="
}

function Test-RequiredFile {
  param([string]$RelativePath)
  $path = Join-Path $ProjectRoot $RelativePath
  if (-not (Test-Path -LiteralPath $path)) {
    throw "Missing required package file: $RelativePath"
  }
}

function Invoke-ScriptChecked {
  param(
    [string]$Path,
    [string[]]$Arguments
  )
  & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $Path @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "Command failed: $Path $($Arguments -join ' ')"
  }
}

Write-Step "Portable package structure"
$required = @(
  "claude-science-assistant.exe",
  "manifest.json",
  "proxy.py",
  "requirements.txt",
  "config.example.json",
  "static\dashboard.html",
  "tests\test_translation.py",
  "scripts\self-test.ps1",
  "scripts\acceptance-v0.1.bat",
  "scripts\collect-acceptance-evidence.ps1",
  "scripts\collect-acceptance-evidence.bat",
  "scripts\start-claude-science-wsl.ps1",
  "scripts\start-claude-science-wsl.sh",
  "scripts\install-wsl-bridge-service.sh",
  "vendor\claude-science\linux-x64\claude-science",
  "vendor\claude-science\linux-x64\claude-science.sha256",
  "vendor\claude-science\linux-x64\manifest.json",
  "skills\bootstrap-claude-science-wsl\SKILL.md",
  "skills\bootstrap-claude-science-wsl\scripts\inspect-windows.ps1",
  "skills\bootstrap-claude-science-wsl\scripts\repair-approved.ps1",
  "skills\bootstrap-claude-science-wsl\scripts\rollback-approved.ps1",
  "docs\quick-start.zh-CN.md",
  "docs\v0.1-clean-pc-acceptance.zh-CN.md"
)
foreach ($item in $required) {
  Test-RequiredFile $item
}
Write-Host "Package structure OK: $ProjectRoot"

$manifestPath = Join-Path $ProjectRoot "manifest.json"
if (Test-Path -LiteralPath $manifestPath) {
  try {
    $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    Write-Host ("Package manifest: {0} v{1} ({2}), generated {3}" -f $manifest.product, $manifest.version, $manifest.profile, $manifest.generatedAt)
    if ($manifest.security.includesSecrets -ne $false) {
      throw "manifest.security.includesSecrets must be false"
    }
    foreach ($manifestFile in @($manifest.entrypoint, $manifest.acceptanceHelper, $manifest.acceptanceHelperBat, $manifest.evidenceHelper, $manifest.evidenceHelperBat, $manifest.skill) + @($manifest.quickStartBats)) {
      if (-not $manifestFile) {
        throw "manifest is missing a required file pointer"
      }
      Test-RequiredFile ($manifestFile -replace "/", "\")
    }
    foreach ($doc in @($manifest.docs)) {
      if (-not $doc) {
        throw "manifest.docs contains an empty path"
      }
      Test-RequiredFile ($doc -replace "/", "\")
    }
    if (-not $manifest.bundledClaudeScience -or -not $manifest.bundledClaudeScience.path -or -not $manifest.bundledClaudeScience.sha256) {
      throw "manifest.bundledClaudeScience is required"
    }
    Test-RequiredFile ($manifest.bundledClaudeScience.path -replace "/", "\")
    Test-RequiredFile ($manifest.bundledClaudeScience.manifest -replace "/", "\")
  } catch {
    throw "Invalid manifest.json: $($_.Exception.Message)"
  }
} else {
  throw "Missing required package file: manifest.json"
}

$skillScripts = Join-Path $ProjectRoot "skills\bootstrap-claude-science-wsl\scripts"
$inspectWindows = Join-Path $skillScripts "inspect-windows.ps1"
$repair = Join-Path $skillScripts "repair-approved.ps1"
$rollback = Join-Path $skillScripts "rollback-approved.ps1"

Write-Step "Read-only Windows inspection"
Invoke-ScriptChecked -Path $inspectWindows -Arguments @("-ProjectRoot", $ProjectRoot)

if ($ApproveInstall) {
  Write-Step "Approved install/repair"
  $args = @("-ProjectRoot", $ProjectRoot, "-ApproveInstall")
  if ($Distro) { $args += @("-Distro", $Distro) }
  if ($User) { $args += @("-User", $User) }
  if ($InstallWslIfMissing) { $args += "-InstallWslIfMissing" }
  if ($StartServices) { $args += "-StartServices" }
  if ($RunSelfTest) { $args += "-RunSelfTest" }
  Invoke-ScriptChecked -Path $repair -Arguments $args
} else {
  Write-Step "Repair preview only"
  $args = @("-ProjectRoot", $ProjectRoot, "-PlanOnly")
  if ($Distro) { $args += @("-Distro", $Distro) }
  if ($User) { $args += @("-User", $User) }
  Invoke-ScriptChecked -Path $repair -Arguments $args
}

if (-not $SkipRollbackPreview) {
  Write-Step "Rollback preview only"
  $args = @("-ProjectRoot", $ProjectRoot, "-PlanOnly")
  if ($Distro) { $args += @("-Distro", $Distro) }
  if ($User) { $args += @("-User", $User) }
  Invoke-ScriptChecked -Path $rollback -Arguments $args
}

Write-Step "Manual launcher acceptance remains"
Write-Host "Open claude-science-assistant.exe and verify:"
Write-Host "- WSL2, Bridge, Claude Science, and current API Key status cards."
Write-Host "- Start, stop, restart, and open Claude Science do not create duplicate Bridge instances."
Write-Host "- The home screen lists saved API Keys in add order; provider templates appear only inside Add API Key."
Write-Host "- Template order: GLM-5.2, LongCat, DeepSeek, MiniMax, Claude, OpenAI/GPT; OpenCode Go, OpenRouter; built-in relay, custom relay."
Write-Host "- Built-in relay shows https://10521052.xyz/v1 as a third-party relay, not official/trusted."
Write-Host "- Saved API keys show only encrypted status; plaintext and ciphertext are never echoed."
Write-Host "- Add at least two test keys, switch with Use, then verify the active key cannot be deleted until another key is active."
Write-Host "- After adding a non-Claude key, verify WSL runtime config permission is 0600 without printing its contents."

Write-Step "Acceptance helper finished"
if ($ApproveInstall) {
  Write-Host "Install/repair path completed. Continue with the manual launcher checks above."
} else {
  Write-Host "Preview path completed. No install/repair was approved by this helper."
}
