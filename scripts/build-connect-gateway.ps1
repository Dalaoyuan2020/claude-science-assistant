[CmdletBinding()]
param(
  [switch]$SkipTests
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
$SourceDir = Join-Path $ProjectDir "connect-gateway"
$OutputDir = Join-Path $ProjectDir "vendor\csa-connect\linux-x64"
$OutputPath = Join-Path $OutputDir "csa-connect"

if (-not $SkipTests) {
  $previousTestToolchain = $env:GOTOOLCHAIN
  $previousMaxProcs = $env:GOMAXPROCS
  try {
    $env:GOTOOLCHAIN = "go1.25.5"
    $env:GOMAXPROCS = "2"
    & go -C $SourceDir test -p=1 ./...
    if ($LASTEXITCODE -ne 0) {
      throw "Connect Gateway tests failed with exit code $LASTEXITCODE"
    }
  } finally {
    $env:GOTOOLCHAIN = $previousTestToolchain
    $env:GOMAXPROCS = $previousMaxProcs
  }
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$previousGoos = $env:GOOS
$previousGoarch = $env:GOARCH
$previousCgo = $env:CGO_ENABLED
$previousToolchain = $env:GOTOOLCHAIN
$previousMaxProcs = $env:GOMAXPROCS
try {
  $env:GOTOOLCHAIN = "go1.25.5"
  $env:GOMAXPROCS = "2"
  $env:GOOS = "linux"
  $env:GOARCH = "amd64"
  $env:CGO_ENABLED = "0"
  & go -C $SourceDir build -p=1 -trimpath -ldflags "-s -w -buildid=" -o $OutputPath ./cmd/csa-connect
  if ($LASTEXITCODE -ne 0) {
    throw "Connect Gateway build failed with exit code $LASTEXITCODE"
  }
} finally {
  $env:GOOS = $previousGoos
  $env:GOARCH = $previousGoarch
  $env:CGO_ENABLED = $previousCgo
  $env:GOTOOLCHAIN = $previousToolchain
  $env:GOMAXPROCS = $previousMaxProcs
}

$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $OutputPath).Hash.ToLowerInvariant()
$manifest = [ordered]@{
  schemaVersion = 1
  name = "csa-connect"
  version = "0.1.0"
  platform = "linux-x64"
  sha256 = $hash
  builtAt = (Get-Date).ToUniversalTime().ToString("o")
}
$manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $OutputDir "manifest.json") -Encoding UTF8
$hash | Set-Content -LiteralPath (Join-Path $OutputDir "csa-connect.sha256") -Encoding ASCII

Write-Host "Built $OutputPath"
Write-Host "SHA256 $hash"
