[CmdletBinding()]
param(
  [string]$ProjectRoot = "",
  [string]$OutputDir = "",
  [string]$Distro = "",
  [string]$User = "",
  [switch]$SkipAcceptancePreview,
  [switch]$SkipWslStatus
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $ProjectRoot) {
  $ProjectRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
} else {
  $ProjectRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
}

if (-not $OutputDir) {
  $OutputDir = Join-Path $ProjectRoot "acceptance-evidence"
}
$OutputDir = (New-Item -ItemType Directory -Force -Path $OutputDir).FullName

$ManifestPath = Join-Path $ProjectRoot "manifest.json"
$IsPortablePackage = (Test-Path -LiteralPath (Join-Path $ProjectRoot "claude-science-assistant.exe")) -and (Test-Path -LiteralPath $ManifestPath)

$Stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$EvidenceName = "claude-science-assistant-v0.1-evidence-$Stamp"
$EvidenceRoot = Join-Path $OutputDir $EvidenceName
New-Item -ItemType Directory -Force -Path $EvidenceRoot | Out-Null

function ConvertTo-RedactedText {
  param([AllowNull()][string]$Text)
  if ($null -eq $Text) {
    return ""
  }

  $value = $Text -replace "`0", ""
  $value = $value -replace 'sk-[A-Za-z0-9_-]{12,}', 'sk-[REDACTED]'
  $value = $value -replace '(?i)(authorization\s*[:=]\s*bearer\s+)[A-Za-z0-9._~+/\-=]{8,}', '$1[REDACTED]'
  $value = $value -replace '(?i)((api|api_key|apikey|oauth|token|secret|password)[A-Za-z0-9_\- ]{0,24}\s*[:=]\s*)[^,\s}]{6,}', '$1[REDACTED]'
  return $value
}

function Save-RedactedText {
  param(
    [string]$RelativePath,
    [AllowNull()][string]$Text
  )

  $path = Join-Path $EvidenceRoot $RelativePath
  $parent = Split-Path -Parent $path
  if ($parent -and -not (Test-Path -LiteralPath $parent)) {
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
  }
  Set-Content -LiteralPath $path -Value (ConvertTo-RedactedText $Text) -Encoding UTF8
  return $RelativePath
}

function Invoke-CapturedNative {
  param(
    [string]$Name,
    [string]$FilePath,
    [string[]]$Arguments
  )

  Write-Host "Collecting: $Name"
  $lines = @()
  $exitCode = 0
  $previousErrorActionPreference = $ErrorActionPreference
  try {
    $ErrorActionPreference = "Continue"
    $lines = @(& $FilePath @Arguments 2>&1 | ForEach-Object {
      if ($_ -is [System.Management.Automation.ErrorRecord]) {
        $_.Exception.Message
      } else {
        "$_"
      }
    })
    $exitCode = if ($null -ne $LASTEXITCODE) { $LASTEXITCODE } else { 0 }
  } catch {
    $lines += $_.Exception.Message
    $exitCode = 1
  } finally {
    $ErrorActionPreference = $previousErrorActionPreference
  }

  $safeName = $Name -replace '[^A-Za-z0-9._-]', '_'
  $logPath = "logs\$safeName.log"
  Save-RedactedText -RelativePath $logPath -Text ($lines -join [Environment]::NewLine) | Out-Null

  return [ordered]@{
    name = $Name
    exitCode = $exitCode
    log = $logPath
  }
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

function Copy-IfPresent {
  param(
    [string]$SourcePath,
    [string]$RelativeDestination
  )

  if (Test-Path -LiteralPath $SourcePath) {
    $destination = Join-Path $EvidenceRoot $RelativeDestination
    $parent = Split-Path -Parent $destination
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
      New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    Copy-Item -LiteralPath $SourcePath -Destination $destination -Force
    return $RelativeDestination
  }

  return $null
}

$summary = [ordered]@{
  schemaVersion = 1
  product = "Claude Science Assistant"
  version = "0.1.3"
  generatedAt = (Get-Date).ToUniversalTime().ToString("o")
  mode = "read-only evidence collection"
  projectRootLeaf = Split-Path -Leaf $ProjectRoot
  portablePackageRoot = $IsPortablePackage
  privacy = "API keys, OAuth tokens, control tokens, bearer tokens, and sk-* tokens are not intentionally collected and logs are redacted before archiving."
  commands = @()
  copiedFiles = @()
}

$copiedManifest = Copy-IfPresent -SourcePath $ManifestPath -RelativeDestination "package\manifest.json"
if ($copiedManifest) {
  $summary.copiedFiles += $copiedManifest
}

$packageLeaf = Split-Path -Leaf $ProjectRoot
$shaCandidates = @(
  (Join-Path $ProjectRoot "$packageLeaf.zip.sha256"),
  (Join-Path (Split-Path -Parent $ProjectRoot) "$packageLeaf.zip.sha256")
)
foreach ($candidate in $shaCandidates) {
  $copiedSha = Copy-IfPresent -SourcePath $candidate -RelativeDestination "package\package.zip.sha256"
  if ($copiedSha) {
    $summary.copiedFiles += $copiedSha
    break
  }
}

$inspectWindows = Join-Path $ProjectRoot "skills\bootstrap-claude-science-wsl\scripts\inspect-windows.ps1"
if (Test-Path -LiteralPath $inspectWindows) {
  $summary.commands += Invoke-CapturedNative -Name "inspect-windows" -FilePath "powershell.exe" -Arguments @(
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    $inspectWindows,
    "-ProjectRoot",
    $ProjectRoot
  )
}

$acceptance = Join-Path $ProjectRoot "scripts\acceptance-v0.1.ps1"
if ((-not $SkipAcceptancePreview) -and (Test-Path -LiteralPath $acceptance)) {
  if ($IsPortablePackage) {
    $acceptanceArgs = @(
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      $acceptance,
      "-ProjectRoot",
      $ProjectRoot
    )
    if ($Distro) {
      $acceptanceArgs += @("-Distro", $Distro)
    }
    if ($User) {
      $acceptanceArgs += @("-User", $User)
    }
    $summary.commands += Invoke-CapturedNative -Name "acceptance-preview" -FilePath "powershell.exe" -Arguments $acceptanceArgs
  } else {
    $skipLog = Save-RedactedText -RelativePath "logs\acceptance-preview-skipped.log" -Text "Skipped because ProjectRoot is not a portable package root. Run from the extracted release package directory, or pass -ProjectRoot to that directory, to execute package acceptance preview."
    $summary.commands += [ordered]@{
      name = "acceptance-preview"
      status = "skipped"
      reason = "ProjectRoot is not a portable package root."
      log = $skipLog
    }
  }
}

if (-not $SkipWslStatus) {
  if (-not $Distro) {
    $Distro = Select-Distro -Distros @(Get-Distros)
  }
  if (-not $Distro) {
    $summary.commands += [ordered]@{
      name = "wsl-runtime-status-no-config-content"
      status = "skipped"
      reason = "No usable WSL distro found."
    }
  } else {
  $wslArgs = @(
    "-d",
    $Distro,
    "--",
    "bash",
    "-lc",
    "if test -e ~/.claude-science/proxy/config.json; then stat -c 'config_exists=true mode=%a owner=%U group=%G path=%n' ~/.claude-science/proxy/config.json; else echo config_exists=false; fi; systemctl --user is-active claude-science-bridge.service 2>/dev/null || true"
  )
  $summary.commands += Invoke-CapturedNative -Name "wsl-runtime-status-no-config-content" -FilePath "wsl.exe" -Arguments $wslArgs
  }
}

Save-RedactedText -RelativePath "summary.json" -Text ($summary | ConvertTo-Json -Depth 6) | Out-Null

$secretMatches = @(Get-ChildItem -LiteralPath $EvidenceRoot -Recurse -File | Select-String -Pattern 'sk-[A-Za-z0-9_-]{20,}' -List -ErrorAction SilentlyContinue)
if ($secretMatches.Count -gt 0) {
  throw "Evidence bundle still contains secret-like sk-* tokens after redaction; refusing to archive."
}

$zipPath = Join-Path $OutputDir "$EvidenceName.zip"
if (Test-Path -LiteralPath $zipPath) {
  Remove-Item -LiteralPath $zipPath -Force
}
Compress-Archive -Path (Join-Path $EvidenceRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal

Write-Host ""
Write-Host "Evidence bundle created:"
Write-Host $zipPath
Write-Host "Review the bundle before sharing. It should not contain API keys or token values."
