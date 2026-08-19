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
