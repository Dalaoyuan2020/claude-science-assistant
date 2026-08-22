[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ProjectDir = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
. (Join-Path $ProjectDir "scripts\package-policy.ps1")

function Assert-Throws {
  param(
    [Parameter(Mandatory = $true)]
    [scriptblock]$Action,
    [Parameter(Mandatory = $true)]
    [string]$ExpectedMessage
  )

  try {
    & $Action
  } catch {
    if ($_.Exception.Message -notlike "*$ExpectedMessage*") {
      throw "Expected error containing '$ExpectedMessage', got '$($_.Exception.Message)'."
    }
    return
  }
  throw "Expected action to fail with '$ExpectedMessage'."
}

$clean = [pscustomobject]@{ Commit = "abc123"; Tree = "def456"; Dirty = $false }
$dirty = [pscustomobject]@{ Commit = "abc123"; Tree = "def456"; Dirty = $true }

Assert-CsaPackageSourcePolicy -SourceState $clean -Profile release
Assert-CsaPackageSourcePolicy -SourceState $clean -Profile debug
Assert-CsaPackageSourcePolicy -SourceState $dirty -Profile debug -AllowDirtySource $true
Assert-Throws -ExpectedMessage "requires a clean Git worktree" -Action {
  Assert-CsaPackageSourcePolicy -SourceState $dirty -Profile release
}
Assert-Throws -ExpectedMessage "debug-only escape hatch" -Action {
  Assert-CsaPackageSourcePolicy -SourceState $clean -Profile release -AllowDirtySource $true
}
Assert-Throws -ExpectedMessage "refused a dirty Git worktree" -Action {
  Assert-CsaPackageSourcePolicy -SourceState $dirty -Profile debug
}

$packageScriptPath = Join-Path $ProjectDir "scripts\package-launcher-portable.ps1"
if (Test-Path -LiteralPath $packageScriptPath) {
  $tokens = $null
  $parseErrors = $null
  $packageAst = [System.Management.Automation.Language.Parser]::ParseFile(
    $packageScriptPath,
    [ref]$tokens,
    [ref]$parseErrors
  )
  if ($parseErrors.Count -gt 0) {
    throw "Portable packaging script has PowerShell parse errors."
  }
  $nameFunction = $packageAst.Find(
    {
      param($node)
      $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq "Get-CsaPortablePackageName"
    },
    $true
  )
  if (-not $nameFunction) {
    throw "Portable package naming function is missing."
  }
  Invoke-Expression $nameFunction.Extent.Text
  $unqualifiedName = Get-CsaPortablePackageName -Version "0.1.5" -BuildProfile "release"
  $qualityName = Get-CsaPortablePackageName -Version "0.1.5" -Qualifier "quality" -BuildProfile "release"
  if ($unqualifiedName -eq $qualityName -or $qualityName -ne "claude-science-assistant-v0.1.5-quality-release-portable") {
    throw "A qualified release must use a distinct artifact name and must not overwrite the unqualified v0.1.5 release."
  }
  $packageScriptText = Get-Content -LiteralPath $packageScriptPath -Raw -Encoding UTF8
  foreach ($literalCleanup in @(
    'Remove-Item -LiteralPath $PackageRoot',
    'Remove-Item -LiteralPath $ZipPath',
    'Remove-Item -LiteralPath $ShaPath'
  )) {
    if (-not $packageScriptText.Contains($literalCleanup)) {
      throw "Portable packaging cleanup must remain limited to exact qualified artifact paths: $literalCleanup"
    }
  }
  $releaseGateReferences = [regex]::Matches($packageScriptText, '"release-gate\.ps1"').Count
  if ($releaseGateReferences -ne 1) {
    throw "release-gate.ps1 must remain a build-time-only dependency and must not be copied into the portable package."
  }
  $acceptanceText = Get-Content -LiteralPath (Join-Path $ProjectDir "scripts\acceptance-v0.1.ps1") -Raw -Encoding UTF8
  if ($acceptanceText.Contains('scripts\release-gate.ps1')) {
    throw "Portable acceptance must not require the build-only release gate."
  }
  foreach ($portableRuntimeFile in @(
    '"csa-network-quality.py"',
    '"test_network_quality.py"',
    '"runtime_activation_integration_test.sh"',
    '"runtime_network_contract_functions_test.sh"',
    '"runtime_lifecycle_20cycle_test.sh"'
  )) {
    if (-not $packageScriptText.Contains($portableRuntimeFile)) {
      throw "Portable package is missing a required runtime quality helper/test: $portableRuntimeFile"
    }
  }
}

$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ("csa-package-policy-{0}.json" -f [guid]::NewGuid().ToString("N"))
try {
  Write-CsaUtf8NoBom -Path $temporary -Content "{`"schemaVersion`":1}`n"
  Assert-CsaUtf8NoBom -Path $temporary
  $bytes = [System.IO.File]::ReadAllBytes($temporary)
  if ($bytes[0] -ne [byte][char]'{' -or ((Get-Content -LiteralPath $temporary -Raw | ConvertFrom-Json).schemaVersion) -ne 1) {
    throw "No-BOM JSON writer did not produce valid JSON."
  }
} finally {
  Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
}

Write-Host "package policy tests passed"
