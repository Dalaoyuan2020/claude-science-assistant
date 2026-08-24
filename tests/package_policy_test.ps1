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
  $unqualifiedName = Get-CsaPortablePackageName -Version "0.1.6" -BuildProfile "release"
  $qualityName = Get-CsaPortablePackageName -Version "0.1.6" -Qualifier "quality" -BuildProfile "release"
  if ($unqualifiedName -eq $qualityName -or $qualityName -ne "claude-science-assistant-v0.1.6-quality-release-portable") {
    throw "A qualified release must use a distinct artifact name and must not overwrite the unqualified v0.1.6 release."
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
  $selfTestText = Get-Content -LiteralPath (Join-Path $ProjectDir "scripts\self-test.ps1") -Raw -Encoding UTF8
  foreach ($portablePolicyText in @($packageScriptText, $acceptanceText, $selfTestText)) {
    if ($portablePolicyText.Contains('forward-443.py')) {
      throw "The legacy privileged-port forwarder must remain excluded from the supported portable package path."
    }
  }
  foreach ($requiredPolicyFile in @('"LICENSE"', '"SECURITY.md"')) {
    if (-not $packageScriptText.Contains($requiredPolicyFile) -or -not $acceptanceText.Contains($requiredPolicyFile.Trim('"'))) {
      throw "Portable package policy is missing $requiredPolicyFile."
    }
  }
  foreach ($portableRuntimeFile in @(
    '"csa-network-quality.py"',
    '"test_network_quality.py"',
    '"unix_socket_adapter_integration_test.py"',
    '"runtime_activation_integration_test.sh"',
    '"runtime_network_contract_functions_test.sh"',
    '"runtime_lifecycle_20cycle_test.sh"'
  )) {
    if (-not $packageScriptText.Contains($portableRuntimeFile)) {
      throw "Portable package is missing a required runtime quality helper/test: $portableRuntimeFile"
    }
  }

  $cacheCleanupFunction = $packageAst.Find(
    {
      param($node)
      $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq "Remove-CsaPackageCacheDirectories"
    },
    $true
  )
  if (-not $cacheCleanupFunction) {
    throw "Portable packaging must define cache-directory cleanup before archiving."
  }
  Invoke-Expression $cacheCleanupFunction.Extent.Text
  foreach ($forbiddenCacheName in @('__pycache__', '.pytest_cache')) {
    if (-not $packageScriptText.Contains(('"{0}"' -f $forbiddenCacheName))) {
      throw "Portable packaging policy is missing forbidden cache directory: $forbiddenCacheName"
    }
  }

  $cacheFixture = Join-Path ([System.IO.Path]::GetTempPath()) ("csa-package-cache-{0}" -f [guid]::NewGuid().ToString("N"))
  try {
    $pythonCache = Join-Path $cacheFixture "scripts\__pycache__"
    $pytestCache = Join-Path $cacheFixture "tests\.pytest_cache"
    New-Item -ItemType Directory -Force -Path $pythonCache, $pytestCache | Out-Null
    Set-Content -LiteralPath (Join-Path $pythonCache "module.pyc") -Value "cache"
    Set-Content -LiteralPath (Join-Path $pytestCache "nodeids") -Value "cache"
    Set-Content -LiteralPath (Join-Path $cacheFixture "keep.txt") -Value "keep"

    Remove-CsaPackageCacheDirectories -PackageRoot $cacheFixture

    if ((Test-Path -LiteralPath $pythonCache) -or (Test-Path -LiteralPath $pytestCache)) {
      throw "Portable cache cleanup left a forbidden cache directory in the package fixture."
    }
    if (-not (Test-Path -LiteralPath (Join-Path $cacheFixture "keep.txt"))) {
      throw "Portable cache cleanup removed a non-cache package file."
    }
  } finally {
    Remove-Item -LiteralPath $cacheFixture -Recurse -Force -ErrorAction SilentlyContinue
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
