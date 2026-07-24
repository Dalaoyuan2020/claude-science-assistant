[CmdletBinding()]
param(
  [switch]$LiveExternalAgent,
  [switch]$LiveConnectRuntime,
  [switch]$VerifyProxy,
  [string]$Distro = "Ubuntu-24.04",
  [string]$EvidenceDir = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
if (-not $EvidenceDir) {
  $EvidenceDir = Join-Path $ProjectDir "reports\release-readiness"
}
$EvidenceDir = (New-Item -ItemType Directory -Force -Path $EvidenceDir).FullName
$Cases = New-Object System.Collections.Generic.List[object]
$CargoTargetDir = Join-Path $ProjectDir "launcher\src-tauri\target-release-verify"

function Convert-ToWslPath {
  param([Parameter(Mandatory = $true)][string]$Path)
  $full = [IO.Path]::GetFullPath($Path)
  if ($full -notmatch '^([A-Za-z]):\\(.*)$') {
    throw "Only absolute Windows drive paths can be mapped into WSL: $full"
  }
  $drive = $Matches[1].ToLowerInvariant()
  $tail = $Matches[2].Replace('\', '/')
  return "/mnt/$drive/$tail"
}

function Stop-TestProcessTree {
  param([Parameter(Mandatory = $true)][int]$ProcessId)
  $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $ProcessId" -ErrorAction SilentlyContinue)
  foreach ($child in $children) {
    Stop-TestProcessTree -ProcessId ([int]$child.ProcessId)
  }
  Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
}

function Invoke-ClaudeJsonTurn {
  param(
    [Parameter(Mandatory = $true)][string[]]$Arguments,
    [int]$TimeoutSeconds = 120
  )
  $command = Get-Command claude.cmd -ErrorAction SilentlyContinue
  if (-not $command) { throw "Claude Code CLI is not installed or not on PATH." }
  $quotedArguments = @($Arguments | ForEach-Object {
    if ($_ -match '[\s"]') { '"' + $_.Replace('"', '\"') + '"' } else { $_ }
  })
  $commandLine = '"' + $command.Source + '" ' + ($quotedArguments -join ' ')
  $commandProcessorArguments = '/d /s /c "' + $commandLine + '"'
  $process = $null
  try {
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $env:ComSpec
    $startInfo.Arguments = $commandProcessorArguments
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "Unable to start Claude Code live verification." }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
      Stop-TestProcessTree -ProcessId $process.Id
      throw "Claude Code live verification timed out after $TimeoutSeconds seconds."
    }
    $process.WaitForExit()
    $exitCode = $process.ExitCode
    $raw = $stdoutTask.GetAwaiter().GetResult()
    $null = $stderrTask.GetAwaiter().GetResult()
    if ($exitCode -ne 0) {
      throw "Claude Code live verification failed with exit code $exitCode."
    }
    return ($raw.TrimStart([char]0xFEFF).Trim() | ConvertFrom-Json)
  } finally {
    if ($process) { $process.Dispose() }
  }
}

function Invoke-Case {
  param(
    [Parameter(Mandatory = $true)][string]$Id,
    [Parameter(Mandatory = $true)][string]$Description,
    [Parameter(Mandatory = $true)][scriptblock]$Action
  )
  $started = [Diagnostics.Stopwatch]::StartNew()
  try {
    & $Action
    $started.Stop()
    $Cases.Add([ordered]@{
      id = $Id
      description = $Description
      status = "passed"
      durationMs = $started.ElapsedMilliseconds
    })
    Write-Host "PASS $Id - $Description"
  } catch {
    $started.Stop()
    $Cases.Add([ordered]@{
      id = $Id
      description = $Description
      status = "failed"
      durationMs = $started.ElapsedMilliseconds
      error = [string]$_.Exception.Message
    })
    Write-Host "FAIL $Id - $Description"
    throw
  }
}

Invoke-Case "SA-01" "Sandbox request script writes a manual-approval inbox envelope" {
  $fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ("csa-subagent-fixture-" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null
  try {
    & (Join-Path $ScriptDir "create-subagent-request.ps1") `
      -ProjectRoot $fixtureRoot `
      -TaskKind environment `
      -Title "Release verification environment diagnosis" `
      -Note "Read-only dependency diagnosis with redacted errors." `
      -RequestedAction diagnose `
      -ApprovalMode manual `
      -PolicyId manual-only | Out-Null
    $files = @(Get-ChildItem -LiteralPath (Join-Path $fixtureRoot "reports\csa-agent-inbox") -Filter "*.json" -File)
    if ($files.Count -ne 1) { throw "Expected exactly one inbox request, found $($files.Count)." }
    $request = Get-Content -LiteralPath $files[0].FullName -Raw -Encoding UTF8 | ConvertFrom-Json
    if (
      [int]$request.schemaVersion -ne 1 -or
      [string]$request.taskKind -ne "environment" -or
      [string]$request.requestedAction -ne "diagnose" -or
      [string]$request.approvalMode -ne "manual" -or
      [string]$request.policyId -ne "manual-only"
    ) {
      throw "Inbox request schema or approval policy is invalid."
    }

    $credentialRejected = $false
    try {
      & (Join-Path $ScriptDir "create-subagent-request.ps1") `
        -ProjectRoot $fixtureRoot `
        -Title "Unsafe fixture" `
        -Note "API_KEY=not-a-real-test-value" | Out-Null
    } catch {
      $credentialRejected = $true
    }
    if (-not $credentialRejected) { throw "Credential-like request content was not rejected." }
  } finally {
    $resolvedTemp = [IO.Path]::GetFullPath($fixtureRoot)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ($resolvedTemp.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
      Remove-Item -LiteralPath $resolvedTemp -Recurse -Force -ErrorAction SilentlyContinue
    }
  }
}

Invoke-Case "SA-02" "Subagent safety, outbox, session, and redaction unit tests pass" {
  Push-Location (Join-Path $ProjectDir "launcher\src-tauri")
  try {
    $previousTarget = $env:CARGO_TARGET_DIR
    $env:CARGO_TARGET_DIR = $CargoTargetDir
    & cargo test --jobs 1 subagent_
    if ($LASTEXITCODE -ne 0) { throw "Subagent Rust tests failed with exit code $LASTEXITCODE." }
  } finally {
    $env:CARGO_TARGET_DIR = $previousTarget
    Pop-Location
  }
}

Invoke-Case "SA-03" "Claude Science External Agent Skill submits and reads a stable outbox result" {
  $fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) ("csa-external-skill-" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null
  try {
    $submitWindows = Join-Path $ProjectDir "skills\csa-external-agent\scripts\submit-request.sh"
    $readWindows = Join-Path $ProjectDir "skills\csa-external-agent\scripts\read-result.sh"
    $submitWsl = Convert-ToWslPath $submitWindows
    $readWsl = Convert-ToWslPath $readWindows
    $fixtureWsl = Convert-ToWslPath $fixtureRoot
    if (-not $submitWsl -or -not $readWsl -or -not $fixtureWsl) {
      throw "Unable to map External Agent Skill paths into WSL."
    }

    $requestId = (& wsl.exe -d $Distro -- bash $submitWsl `
      --project-root $fixtureWsl `
      --task-kind dataset `
      --title "Dataset access diagnosis" `
      --note "Read-only diagnosis with redacted errors." | Select-Object -Last 1).Trim()
    if ($LASTEXITCODE -ne 0 -or $requestId -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$') {
      throw "External Agent Skill did not return a valid request ID."
    }
    $requestPath = Join-Path $fixtureRoot "reports\csa-agent-inbox\$requestId.json"
    $request = Get-Content -LiteralPath $requestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([string]$request.approvalMode -ne "manual" -or [string]$request.policyId -ne "manual-only") {
      throw "External Agent Skill request bypassed manual approval."
    }

    $outboxDir = New-Item -ItemType Directory -Force -Path (Join-Path $fixtureRoot "reports\csa-agent-outbox")
    [ordered]@{
      schemaVersion = 1
      requestId = $requestId
      status = "completed"
      latestRunId = "$requestId-run"
      sessionId = "session-redacted"
      resultPath = "reports/csa-agent-runs/$requestId-run/result.json"
      summary = "Read-only diagnosis completed."
      nextAction = "read_result"
      updatedAt = (Get-Date).ToUniversalTime().ToString("o")
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $outboxDir.FullName "$requestId.json") -Encoding UTF8

    $resultRaw = & wsl.exe -d $Distro -- bash $readWsl --project-root $fixtureWsl --request-id $requestId
    if ($LASTEXITCODE -ne 0) { throw "External Agent Skill could not read its outbox result." }
    $result = ($resultRaw -join "`n") | ConvertFrom-Json
    if ([string]$result.status -ne "completed" -or [string]$result.requestId -ne $requestId) {
      throw "External Agent Skill returned the wrong outbox result."
    }
  } finally {
    $resolvedTemp = [IO.Path]::GetFullPath($fixtureRoot)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ($resolvedTemp.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
      Remove-Item -LiteralPath $resolvedTemp -Recurse -Force -ErrorAction SilentlyContinue
    }
  }
}

Invoke-Case "CN-01" "Connect Gateway queue, pairing, deduplication, media, and MCP tests pass" {
  Push-Location (Join-Path $ProjectDir "connect-gateway")
  try {
    $previousGoMax = $env:GOMAXPROCS
    $env:GOMAXPROCS = "2"
    & go test ./...
    if ($LASTEXITCODE -ne 0) { throw "Connect Gateway tests failed with exit code $LASTEXITCODE." }
  } finally {
    $env:GOMAXPROCS = $previousGoMax
    Pop-Location
  }
}

Invoke-Case "CN-02" "Browser connector is local-only and JavaScript is valid" {
  $extensionDir = Join-Path $ProjectDir "extensions\csa-claude-science-connector"
  foreach ($file in @("content.js", "background.js", "popup.js")) {
    & node --check (Join-Path $extensionDir $file)
    if ($LASTEXITCODE -ne 0) { throw "Browser extension syntax check failed: $file" }
  }
  $manifest = Get-Content -LiteralPath (Join-Path $extensionDir "manifest.json") -Raw -Encoding UTF8 | ConvertFrom-Json
  $hosts = @($manifest.host_permissions)
  if ($hosts -contains "<all_urls>") { throw "Browser extension requests all_urls." }
  $unexpected = @($hosts | Where-Object { $_ -notmatch '^http://(localhost|127\.0\.0\.1):(8765|9882)/\*$' })
  if ($unexpected.Count -gt 0) { throw "Browser extension contains an unexpected host permission." }
}

Invoke-Case "CORE-01" "Launcher frontend compiles" {
  Push-Location (Join-Path $ProjectDir "launcher")
  try {
    & pnpm build
    if ($LASTEXITCODE -ne 0) { throw "Launcher frontend build failed with exit code $LASTEXITCODE." }
  } finally {
    Pop-Location
  }
}

Invoke-Case "CORE-02" "Full launcher Rust suite passes with low concurrency" {
  Push-Location (Join-Path $ProjectDir "launcher\src-tauri")
  try {
    $previousTarget = $env:CARGO_TARGET_DIR
    $env:CARGO_TARGET_DIR = $CargoTargetDir
    & cargo test --jobs 1
    if ($LASTEXITCODE -ne 0) { throw "Launcher Rust tests failed with exit code $LASTEXITCODE." }
  } finally {
    $env:CARGO_TARGET_DIR = $previousTarget
    Pop-Location
  }
}

Invoke-Case "CORE-03" "Bridge translation and safety self-test passes" {
  & (Join-Path $ScriptDir "self-test.ps1")
  if ($LASTEXITCODE -ne 0) { throw "self-test.ps1 failed with exit code $LASTEXITCODE." }
}

Invoke-Case "PKG-01" "Versioned portable package inventory includes both new feature stacks" {
  $versions = @(
    (Get-Content -LiteralPath (Join-Path $ProjectDir "launcher\package.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version,
    (Get-Content -LiteralPath (Join-Path $ProjectDir "launcher\src-tauri\tauri.conf.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version
  )
  $cargo = Get-Content -LiteralPath (Join-Path $ProjectDir "launcher\src-tauri\Cargo.toml") -Raw -Encoding UTF8
  $cargoMatch = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"')
  if (-not $cargoMatch.Success) { throw "Unable to read Cargo launcher version." }
  $versions += $cargoMatch.Groups[1].Value
  if (@($versions | Select-Object -Unique).Count -ne 1) { throw "Launcher versions disagree." }

  foreach ($path in @(
    "connect-gateway\go.mod",
    "extensions\csa-claude-science-connector\manifest.json",
    "skills\csa-connect\SKILL.md",
    "skills\csa-external-agent\SKILL.md",
    "scripts\create-subagent-request.ps1",
    "scripts\create-subagent-request.sh",
    "scripts\verify-new-features.ps1",
    "docs\github-release-v$($versions[0]).md",
    "docs\v0.2-new-features-acceptance.zh-CN.md",
    "docs\v0.2-technology-and-release-review.zh-CN.md",
    "docs\v0.2-install-upgrade-release-guide.zh-CN.md"
  )) {
    if (-not (Test-Path -LiteralPath (Join-Path $ProjectDir $path))) {
      throw "Required release artifact is missing: $path"
    }
  }
}

if ($LiveExternalAgent) {
  Invoke-Case "SA-LIVE-01" "Claude Code executes and resumes a real two-turn plan-mode session" {
    $session = [guid]::NewGuid().ToString()
    $contextToken = "CSA-CONTEXT-" + (Get-Random -Minimum 100000 -Maximum 999999)
    $first = Invoke-ClaudeJsonTurn -Arguments @(
      "--session-id", $session,
      "--permission-mode", "plan",
      "-p", "Remember $contextToken. Reply only: FIRST-OK.",
      "--output-format", "json"
    )
    if ([string]$first.result -notmatch "FIRST-OK") { throw "Claude Code first result did not match." }
    $second = Invoke-ClaudeJsonTurn -Arguments @(
      "--resume", $session,
      "--permission-mode", "plan",
      "-p", "Reply only with the token I asked you to remember.",
      "--output-format", "json"
    )
    if ([string]$second.result -notmatch [regex]::Escape($contextToken)) {
      throw "Claude Code session context was not preserved."
    }
  }
}

if ($LiveConnectRuntime) {
  Invoke-Case "CN-LIVE-01" "Running Connect Gateway exposes a ready local MCP queue" {
    $statusCommand = '$HOME/.local/share/claude-science-api-bridge/bin/csa-connect status --config $HOME/.local/share/claude-science-api-bridge/connect/config.json'
    $statusRaw = & wsl.exe -d $Distro -- sh -lc $statusCommand
    if ($LASTEXITCODE -ne 0) { throw "Unable to read Connect Gateway status." }
    $status = ($statusRaw -join "`n") | ConvertFrom-Json
    if (-not $status.running -or -not $status.mcpReady) {
      throw "Connect Gateway is not running with MCP ready."
    }
    if ([string]$status.mcpUrl -ne "http://127.0.0.1:9881/mcp") {
      throw "Connect Gateway MCP is not bound to the expected loopback URL."
    }
  }
}

if ($VerifyProxy) {
  Invoke-Case "CORE-LIVE-01" "Running Bridge passes live proxy verification" {
    & (Join-Path $ScriptDir "verify-proxy.ps1")
    if ($LASTEXITCODE -ne 0) { throw "verify-proxy.ps1 failed with exit code $LASTEXITCODE." }
  }
}

$failed = @($Cases | Where-Object { $_.status -ne "passed" }).Count
$caseArray = $Cases.ToArray()
$commit = (& git -C $ProjectDir rev-parse HEAD 2>$null | Select-Object -First 1)
$dirty = [bool](& git -C $ProjectDir status --porcelain 2>$null | Select-Object -First 1)
$Evidence = [ordered]@{
  schemaVersion = 1
  generatedAt = (Get-Date).ToUniversalTime().ToString("o")
  sourceCommit = [string]$commit
  sourceTreeDirty = $dirty
  liveExternalAgent = [bool]$LiveExternalAgent
  liveConnectRuntime = [bool]$LiveConnectRuntime
  proxyVerified = [bool]$VerifyProxy
  passed = $Cases.Count - $failed
  failed = $failed
  cases = $caseArray
}
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$evidencePath = Join-Path $EvidenceDir "new-features-$stamp.json"
$temporaryPath = "$evidencePath.tmp"
$Evidence | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $temporaryPath -Encoding UTF8
Move-Item -LiteralPath $temporaryPath -Destination $evidencePath -Force

Write-Host "Evidence: $evidencePath"
if ($failed -gt 0) { exit 1 }
Write-Host "New feature verification passed"
