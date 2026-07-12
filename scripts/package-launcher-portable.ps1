[CmdletBinding()]
param(
  [ValidateSet("debug", "release")]
  [string]$Profile = "debug",
  [string]$OutputDir = "",
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9.-]*$')]
  [string]$PackageQualifier = "",
  [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
$LauncherDir = Join-Path $ProjectDir "launcher"
$CargoToml = Get-Content -LiteralPath (Join-Path $LauncherDir "src-tauri\Cargo.toml") -Raw -Encoding UTF8
$CargoVersionMatch = [regex]::Match($CargoToml, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $CargoVersionMatch.Success) {
  throw "Unable to read launcher version from Cargo.toml."
}
$Version = $CargoVersionMatch.Groups[1].Value
$PackageVersion = (Get-Content -LiteralPath (Join-Path $LauncherDir "package.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version
$TauriVersion = (Get-Content -LiteralPath (Join-Path $LauncherDir "src-tauri\tauri.conf.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version
if ($Version -ne $PackageVersion -or $Version -ne $TauriVersion) {
  throw "Launcher versions disagree: Cargo=$Version package=$PackageVersion tauri=$TauriVersion"
}
if ($Profile -eq "release" -and $SkipBuild) {
  throw "Release packaging must compile the launcher; -SkipBuild is allowed only for debug packages."
}

if (-not $OutputDir) {
  $OutputDir = Join-Path $ProjectDir "dist"
}
$OutputDir = (New-Item -ItemType Directory -Force -Path $OutputDir).FullName

if (-not $SkipBuild) {
  Push-Location $LauncherDir
  try {
    if ($Profile -eq "debug") {
      & pnpm tauri build --debug --no-bundle
    } else {
      & pnpm tauri build --no-bundle
    }
    if ($LASTEXITCODE -ne 0) {
      throw "Tauri build failed with exit code $LASTEXITCODE"
    }
  } finally {
    Pop-Location
  }
}

$TargetProfile = if ($Profile -eq "debug") { "debug" } else { "release" }
$TargetDir = Join-Path (Join-Path (Join-Path $LauncherDir "src-tauri") "target") $TargetProfile
$ExePath = Join-Path $TargetDir "claude-science-assistant.exe"
if (-not (Test-Path -LiteralPath $ExePath)) {
  throw "Launcher exe not found: $ExePath"
}

$QualifiedVersion = if ($PackageQualifier) { "v$Version-$PackageQualifier" } else { "v$Version" }
$PackageName = "claude-science-assistant-$QualifiedVersion-$Profile-portable"
$PackageRoot = Join-Path $OutputDir $PackageName
$ZipPath = Join-Path $OutputDir "$PackageName.zip"
$ShaPath = Join-Path $OutputDir "$PackageName.zip.sha256"

if (Test-Path -LiteralPath $PackageRoot) {
  Remove-Item -LiteralPath $PackageRoot -Recurse -Force
}
if (Test-Path -LiteralPath $ZipPath) {
  Remove-Item -LiteralPath $ZipPath -Force
}
if (Test-Path -LiteralPath $ShaPath) {
  Remove-Item -LiteralPath $ShaPath -Force
}

New-Item -ItemType Directory -Force -Path $PackageRoot | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PackageRoot "docs") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path (Join-Path $PackageRoot "docs") "prompts") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path (Join-Path $PackageRoot "docs") "plans") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PackageRoot "skills") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PackageRoot "scripts") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PackageRoot "static") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PackageRoot "tests") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $PackageRoot "vendor") | Out-Null

Copy-Item -LiteralPath $ExePath -Destination (Join-Path $PackageRoot "claude-science-assistant.exe")
foreach ($file in @("proxy.py", "setup-token.py", "forward-443.py", "requirements.txt", "config.example.json")) {
  Copy-Item -LiteralPath (Join-Path $ProjectDir $file) -Destination (Join-Path $PackageRoot $file)
}
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "static") "dashboard.html") -Destination (Join-Path (Join-Path $PackageRoot "static") "dashboard.html")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "tests") "test_translation.py") -Destination (Join-Path (Join-Path $PackageRoot "tests") "test_translation.py")
foreach ($file in @(
  "install-wsl-bridge-service.sh",
  "start-claude-science-wsl.sh",
  "start-claude-science-wsl.ps1",
  "status-probe.ps1",
  "self-test.ps1",
  "acceptance-v0.1.ps1",
  "acceptance-v0.1.bat",
  "collect-acceptance-evidence.ps1",
  "collect-acceptance-evidence.bat",
  "verify-proxy.ps1",
  "probe-provider-capabilities.ps1",
  "doctor.ps1",
  "uninstall.ps1"
)) {
  Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "scripts") $file) -Destination (Join-Path (Join-Path $PackageRoot "scripts") $file)
}
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "quick-start.zh-CN.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "quick-start.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "architecture-and-product-plan.zh-CN.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "architecture-and-product-plan.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "github-release-v0.1.3.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "github-release-v0.1.3.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "green-book-integration.zh-CN.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "green-book-integration.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "v0.1-requirement-audit.zh-CN.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "v0.1-requirement-audit.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "v0.1-current-pc-verification.zh-CN.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "v0.1-current-pc-verification.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "v0.1.3-update-record.zh-CN.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "v0.1.3-update-record.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "v0.1-clean-pc-acceptance.zh-CN.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "v0.1-clean-pc-acceptance.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "provider-access-matrix.zh-CN.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "provider-access-matrix.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "docs") "troubleshooting.md") -Destination (Join-Path (Join-Path $PackageRoot "docs") "troubleshooting.md")
Copy-Item -LiteralPath (Join-Path (Join-Path (Join-Path $ProjectDir "docs") "prompts") "csa-install-or-upgrade-agent-prompt.zh-CN.md") -Destination (Join-Path (Join-Path (Join-Path $PackageRoot "docs") "prompts") "csa-install-or-upgrade-agent-prompt.zh-CN.md")
Copy-Item -LiteralPath (Join-Path (Join-Path (Join-Path $ProjectDir "docs") "prompts") "csa-wsl-storage-migration-codex-prompt.zh-CN.md") -Destination (Join-Path (Join-Path (Join-Path $PackageRoot "docs") "prompts") "csa-wsl-storage-migration-codex-prompt.zh-CN.md")
foreach ($file in @(
  "wsl-storage-migration-context-checkpoint.zh-CN.md",
  "wsl-storage-migration-plan.zh-CN.md",
  "wsl-storage-migration-review-result.zh-CN.md"
)) {
  Copy-Item -LiteralPath (Join-Path (Join-Path (Join-Path $ProjectDir "docs") "plans") $file) -Destination (Join-Path (Join-Path (Join-Path $PackageRoot "docs") "plans") $file)
}
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "skills") "bootstrap-claude-science-wsl") -Destination (Join-Path (Join-Path $PackageRoot "skills") "bootstrap-claude-science-wsl") -Recurse
$BundledClaudeDir = Join-Path (Join-Path (Join-Path $ProjectDir "vendor") "claude-science") "linux-x64"
$BundledClaudeBin = Join-Path $BundledClaudeDir "claude-science"
$BundledClaudeManifest = Join-Path $BundledClaudeDir "manifest.json"
if (-not (Test-Path -LiteralPath $BundledClaudeBin)) {
  throw "Bundled Claude Science Linux binary is required but missing: $BundledClaudeBin"
}
Copy-Item -LiteralPath (Join-Path (Join-Path $ProjectDir "vendor") "claude-science") -Destination (Join-Path (Join-Path $PackageRoot "vendor") "claude-science") -Recurse
Get-ChildItem -LiteralPath (Join-Path (Join-Path $PackageRoot "vendor") "claude-science") -Recurse -File |
  Where-Object { $_.Extension -in @(".rar", ".zip", ".7z") } |
  Remove-Item -Force
$BundledClaudeInfo = Get-Content -LiteralPath $BundledClaudeManifest -Raw -Encoding UTF8 | ConvertFrom-Json
$BundledClaudeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $BundledClaudeBin).Hash.ToLowerInvariant()
if ([string]$BundledClaudeInfo.sha256 -ne $BundledClaudeHash) {
  throw "Bundled Claude Science hash does not match manifest.json."
}

$ExampleConfig = Get-Content -LiteralPath (Join-Path $ProjectDir "config.example.json") -Raw -Encoding UTF8 | ConvertFrom-Json
foreach ($secretField in @("deepseek_api_key", "openai_api_key", "custom_api_key", "proxy_auth_token")) {
  if (-not [string]::IsNullOrEmpty([string]$ExampleConfig.$secretField)) {
    throw "config.example.json contains a value in $secretField."
  }
}
if (
  -not [string]::IsNullOrEmpty([string]$ExampleConfig.force_model) -or
  @($ExampleConfig.model_aliases).Count -gt 0 -or
  @($ExampleConfig.model_token_caps.PSObject.Properties).Count -gt 0 -or
  @($ExampleConfig.deepseek_model_map.PSObject.Properties).Count -gt 0 -or
  @($ExampleConfig.openai_model_map.PSObject.Properties).Count -gt 0 -or
  @($ExampleConfig.custom_model_map.PSObject.Properties).Count -gt 0 -or
  [int]$ExampleConfig.default_max_tokens_cap -ne 0
) {
  throw "config.example.json must preserve the empty model/output-cap state."
}

$ExeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $ExePath).Hash.ToLowerInvariant()
$SourceCommit = (& git -C $ProjectDir rev-parse HEAD 2>$null | Select-Object -First 1)
if ($LASTEXITCODE -ne 0) { $SourceCommit = "unknown" }
$SourceDirty = $false
if ($SourceCommit -ne "unknown") {
  $SourceDirty = [bool](& git -C $ProjectDir status --porcelain 2>$null | Select-Object -First 1)
}

$Manifest = [ordered]@{
  schemaVersion = 1
  product = "CSA - Claude Science Assistant"
  version = $Version
  packageQualifier = $PackageQualifier
  profile = $Profile
  packageName = $PackageName
  generatedAt = (Get-Date).ToUniversalTime().ToString("o")
  sourceCommit = [string]$SourceCommit
  sourceTreeDirty = $SourceDirty
  entrypoint = "claude-science-assistant.exe"
  entrypointSha256 = $ExeHash
  acceptanceHelper = "scripts/acceptance-v0.1.ps1"
  acceptanceHelperBat = "scripts/acceptance-v0.1.bat"
  evidenceHelper = "scripts/collect-acceptance-evidence.ps1"
  evidenceHelperBat = "scripts/collect-acceptance-evidence.bat"
  statusProbe = "scripts/status-probe.ps1"
  quickStartBats = @(
    "1-run-acceptance-preview.bat",
    "2-collect-acceptance-evidence.bat",
    "3-open-claude-science-assistant.bat",
    "4-install-runtime-after-preview.bat"
  )
  bundledClaudeScience = [ordered]@{
    path = "vendor/claude-science/linux-x64/claude-science"
    manifest = "vendor/claude-science/linux-x64/manifest.json"
    sha256 = $BundledClaudeHash
    version = $BundledClaudeInfo.version
    platform = $BundledClaudeInfo.platform
  }
  skill = "skills/bootstrap-claude-science-wsl/SKILL.md"
  docs = @(
    "docs/quick-start.zh-CN.md",
    "docs/architecture-and-product-plan.zh-CN.md",
    "docs/github-release-v0.1.3.md",
    "docs/green-book-integration.zh-CN.md",
    "docs/v0.1-requirement-audit.zh-CN.md",
    "docs/v0.1-current-pc-verification.zh-CN.md",
    "docs/v0.1.3-update-record.zh-CN.md",
    "docs/v0.1-clean-pc-acceptance.zh-CN.md",
    "docs/provider-access-matrix.zh-CN.md",
    "docs/troubleshooting.md",
    "docs/prompts/csa-install-or-upgrade-agent-prompt.zh-CN.md",
    "docs/prompts/csa-wsl-storage-migration-codex-prompt.zh-CN.md",
    "docs/plans/wsl-storage-migration-context-checkpoint.zh-CN.md",
    "docs/plans/wsl-storage-migration-plan.zh-CN.md",
    "docs/plans/wsl-storage-migration-review-result.zh-CN.md"
  )
  security = [ordered]@{
    includesSecrets = $false
    apiKeysPortable = $false
    apiKeyStorage = "Windows current-user DPAPI for launcher list; active WSL runtime config is chmod 0600"
  }
  expectedRootFiles = @(
    "proxy.py",
    "requirements.txt",
    "scripts/",
    "static/",
    "tests/",
    "docs/",
    "skills/",
    "vendor/"
  )
}
$Manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $PackageRoot "manifest.json") -Encoding UTF8

$Readme = @(
  ("# CSA - Claude Science Assistant v{0} portable package" -f $Version),
  "",
  "Quick start:",
  "1. Double-click 1-run-acceptance-preview.bat to run read-only acceptance preview. It does not install or delete anything.",
  "2. Double-click 2-collect-acceptance-evidence.bat to create a redacted evidence bundle for troubleshooting.",
  "3. Double-click 3-open-claude-science-assistant.bat to open the launcher.",
  "4. If this PC already has a usable WSL/Ubuntu distro, double-click 4-install-runtime-after-preview.bat to install/repair CSA's WSL runtime, start services, and run self-test after confirmation.",
  "5. If this PC does not have WSL/Ubuntu yet, do not treat step 4 as a silent system installer. Installing WSL requires explicit -InstallWslIfMissing, an elevated PowerShell window, and may require a reboot; beginners can ask Codex to run the bundled skill-guided flow.",
  "6. The original PowerShell scripts are still kept in scripts/: acceptance-v0.1.ps1 and collect-acceptance-evidence.ps1.",
  "",
  "Do not copy only the exe. The launcher and skill need proxy.py, requirements.txt, scripts/, static/, tests/, docs/, and skills/ in the same extracted folder.",
  "Add Provider is the model-entry flow. Non-Claude entries require a key, which is encrypted with Windows current-user DPAPI; saved keys can be switched from the ordered list and are never echoed.",
  "For cross-PC diagnostics, run scripts/status-probe.ps1 from the extracted package root. It verifies WSL health, service path relocation, Bridge health, and Claude Science ports without printing secrets.",
  "DPAPI keys are tied to the current Windows user and PC. Copying this portable package to another PC does not carry API keys; add them again on that PC.",
  ("This package bundles locked Claude Science Linux binary {0}, sha256 {1}." -f $BundledClaudeInfo.version, $BundledClaudeInfo.sha256),
  "For Chinese instructions, see docs/quick-start.zh-CN.md, docs/prompts/csa-install-or-upgrade-agent-prompt.zh-CN.md, docs/prompts/csa-wsl-storage-migration-codex-prompt.zh-CN.md, docs/green-book-integration.zh-CN.md, docs/v0.1-clean-pc-acceptance.zh-CN.md, and manifest.json.",
  "",
  "This package does not include API keys, OAuth tokens, control tokens, or user config."
) -join [Environment]::NewLine
Set-Content -LiteralPath (Join-Path $PackageRoot "README.md") -Value $Readme -Encoding UTF8

$RootAcceptanceBat = @(
  "@echo off",
  "setlocal EnableExtensions",
  "call ""%~dp0scripts\acceptance-v0.1.bat"" %*",
  "exit /b %ERRORLEVEL%"
) -join [Environment]::NewLine
Set-Content -LiteralPath (Join-Path $PackageRoot "1-run-acceptance-preview.bat") -Value $RootAcceptanceBat -Encoding ASCII

$RootEvidenceBat = @(
  "@echo off",
  "setlocal EnableExtensions",
  "call ""%~dp0scripts\collect-acceptance-evidence.bat"" %*",
  "exit /b %ERRORLEVEL%"
) -join [Environment]::NewLine
Set-Content -LiteralPath (Join-Path $PackageRoot "2-collect-acceptance-evidence.bat") -Value $RootEvidenceBat -Encoding ASCII

$RootLauncherBat = @(
  "@echo off",
  "setlocal EnableExtensions",
  "set ""EXE=%~dp0claude-science-assistant.exe""",
  "if not exist ""%EXE%"" (",
  "  echo Missing launcher: ""%EXE%""",
  "  pause",
  "  exit /b 1",
  ")",
  "start """" ""%EXE%""",
  "exit /b 0"
) -join [Environment]::NewLine
Set-Content -LiteralPath (Join-Path $PackageRoot "3-open-claude-science-assistant.bat") -Value $RootLauncherBat -Encoding ASCII

$RootInstallBat = @(
  "@echo off",
  "setlocal EnableExtensions",
  "chcp 65001 >nul",
  "set ""ASSUME_YES=""",
  "if /I ""%~1""==""--yes"" (",
  "  set ""ASSUME_YES=1""",
  "  shift /1",
  ")",
  "echo This will install/repair the WSL runtime, install the bundled locked Claude Science Linux binary, start services, and run self-test.",
  "echo Run 1-run-acceptance-preview.bat first. Continue only if the preview looks safe.",
  "echo.",
  "if defined ASSUME_YES goto run_install",
  "choice /C YN /M ""Continue with install/start/self-test?""",
  "if errorlevel 2 exit /b 2",
  ":run_install",
  "if defined ASSUME_YES (",
  "  call ""%~dp0scripts\acceptance-v0.1.bat"" --no-pause -ApproveInstall -StartServices -RunSelfTest",
  "  exit /b %ERRORLEVEL%",
  ")",
  "call ""%~dp0scripts\acceptance-v0.1.bat"" -ApproveInstall -StartServices -RunSelfTest",
  "exit /b %ERRORLEVEL%"
) -join [Environment]::NewLine
Set-Content -LiteralPath (Join-Path $PackageRoot "4-install-runtime-after-preview.bat") -Value $RootInstallBat -Encoding ASCII

$secretMatches = @(Get-ChildItem -LiteralPath $PackageRoot -Recurse -File | Select-String -Pattern 'sk-[A-Za-z0-9_-]{20,}' -List -ErrorAction SilentlyContinue)
if ($secretMatches.Count -gt 0) {
  throw "Package contains secret-like tokens; refusing to archive."
}

$forbiddenPackageFiles = @(
  Get-ChildItem -LiteralPath $PackageRoot -Recurse -File |
    Where-Object {
      $_.FullName -match '[\\/]private[\\/]' -or
      $_.Extension -in @(".rar", ".zip", ".7z")
    }
)
if ($forbiddenPackageFiles.Count -gt 0) {
  throw "Package contains private or nested archive files; refusing to archive."
}

Compress-Archive -Path (Join-Path $PackageRoot "*") -DestinationPath $ZipPath -CompressionLevel Optimal
$Hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $ZipPath).Hash.ToLowerInvariant()
Set-Content -LiteralPath $ShaPath -Value "$Hash  $(Split-Path -Leaf $ZipPath)" -Encoding ASCII

[ordered]@{
  package = $ZipPath
  sha256 = $ShaPath
  profile = $Profile
  exe = $ExePath
} | ConvertTo-Json -Depth 4
