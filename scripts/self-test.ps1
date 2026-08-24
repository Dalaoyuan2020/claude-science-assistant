param(
  [string]$Python = "",
  [switch]$Offline
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = (Resolve-Path (Join-Path $ScriptDir "..")).Path
. (Join-Path $ScriptDir "package-policy.ps1")
Set-Location $ProjectDir

$defaultPython = Join-Path $ProjectDir ".venv\Scripts\python.exe"
if (-not $Python) {
  if (Test-Path -LiteralPath $defaultPython) {
    $Python = $defaultPython
  } elseif ($env:PYTHON) {
    $Python = $env:PYTHON
  } elseif ($Offline) {
    $pythonCommand = Get-Command python -ErrorAction SilentlyContinue
    if ($pythonCommand) {
      $Python = $pythonCommand.Source
    } else {
      throw "Offline self-test requires a prepared project .venv or a usable PYTHON/python interpreter; none was found."
    }
  } else {
    $Python = $defaultPython
  }
}

if (-not (Test-Path -LiteralPath $Python)) {
  $pythonCommand = Get-Command $Python -ErrorAction SilentlyContinue
  if ($pythonCommand) {
    $Python = $pythonCommand.Source
  } elseif ($Offline -or $Python -ne $defaultPython) {
    throw "Python not found: $Python"
  } else {
    $venvDir = Join-Path $ProjectDir ".venv"
    $py = Get-Command py -ErrorAction SilentlyContinue
    $systemPython = Get-Command python -ErrorAction SilentlyContinue
    if ($py) {
      & $py.Source -3 -m venv $venvDir
    } elseif ($systemPython) {
      & $systemPython.Source -m venv $venvDir
    } else {
      throw "Python not found. Install Python 3 or set PYTHON to python.exe."
    }
    if ($LASTEXITCODE -ne 0) { throw "Failed to create Python virtual environment (exit $LASTEXITCODE)." }
    $Python = $defaultPython
  }
}

$Python = (Resolve-Path -LiteralPath $Python).Path
$pythonIdentity = @(& $Python -c "import sys; print('CSA_PYTHON_' + str(sys.version_info[0]))" 2>$null)
if ($LASTEXITCODE -ne 0 -or "CSA_PYTHON_3" -notin $pythonIdentity) {
  throw "Configured interpreter is not a usable Python 3 executable: $Python"
}
& $Python -c "import fastapi, httpx, starlette, uvicorn"
$dependenciesReady = ($LASTEXITCODE -eq 0)
if (-not $dependenciesReady) {
  if ($Offline) {
    throw "Offline self-test cannot install missing runtime dependencies. Prepare $Python from requirements.txt first."
  }
  Write-Host "Python test dependencies are missing or incomplete; repairing the local venv."
  & $Python -m pip install --upgrade pip
  if ($LASTEXITCODE -ne 0) { throw "Failed to install pip (exit $LASTEXITCODE)." }
  & $Python -m pip install -r (Join-Path $ProjectDir "requirements.txt")
  if ($LASTEXITCODE -ne 0) {
    Write-Host "Configured pip index did not provide the locked requirements; retrying this command against official PyPI without changing pip configuration."
    & $Python -m pip install --index-url https://pypi.org/simple -r (Join-Path $ProjectDir "requirements.txt")
  }
  if ($LASTEXITCODE -ne 0) { throw "Failed to install test requirements from the configured index and official PyPI (exit $LASTEXITCODE)." }
  & $Python -c "import fastapi, httpx, starlette, uvicorn"
  if ($LASTEXITCODE -ne 0) { throw "Python requirements were installed but imports still fail (exit $LASTEXITCODE)." }
}

& $Python -c "import pytest"
if ($LASTEXITCODE -ne 0) {
  if ($Offline) {
    throw "Offline self-test cannot install pytest. Prepare $Python from requirements-dev.txt first."
  }
  Write-Host "pytest is missing; installing the locked test requirements."
  & $Python -m pip install -r (Join-Path $ProjectDir "requirements-dev.txt")
  if ($LASTEXITCODE -ne 0) {
    & $Python -m pip install --index-url https://pypi.org/simple -r (Join-Path $ProjectDir "requirements-dev.txt")
  }
  if ($LASTEXITCODE -ne 0) { throw "Failed to install locked test requirements (exit $LASTEXITCODE)." }
}

& $Python -m py_compile proxy.py setup-token.py scripts/csa-network-quality.py scripts/csa-narrow-broad-host-grants.py
if ($LASTEXITCODE -ne 0) { throw "Python syntax check failed (exit $LASTEXITCODE)." }

$RuntimeManifestPath = Join-Path $ProjectDir "vendor\claude-science\linux-x64\manifest.json"
$RuntimeBinaryPath = Join-Path $ProjectDir "vendor\claude-science\linux-x64\claude-science"
$StartScriptPath = Join-Path $ProjectDir "scripts\start-claude-science-wsl.sh"
Assert-CsaUtf8NoBom -Path $RuntimeManifestPath
$RuntimeManifest = Get-Content -LiteralPath $RuntimeManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ([string]$RuntimeManifest.version -ne "0.1.25") {
  throw "v0.1.6 must lock Claude Science stable 0.1.25."
}
if (Test-Path -LiteralPath $RuntimeBinaryPath) {
  $RuntimeHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $RuntimeBinaryPath).Hash.ToLowerInvariant()
  if ($RuntimeHash -ne [string]$RuntimeManifest.sha256) {
    throw "Bundled Claude Science hash does not match manifest.json."
  }

  $LazyMcpBinaryCheck = @'
from pathlib import Path
import sys

data = Path(sys.argv[1]).read_bytes()
old = b"_loadBundledServer(z,O=!1){if(this._bundledTools.has(z))return Promise.resolve();if(this._bundledParked.has(z))return Promise.resolve();"
new_core = b"_loadBundledServer(z,O=!1){if(!O||this._bundledTools.has(z)||this._bundledParked.has(z))return Promise.resolve();"
new = new_core + (b" " * (len(old) - len(new_core)))
git_old = b"p_.pinBaseRootsAfterGrantProjection(),p_.warmGitScan(),p_.setStoreDeniedDomains(TP())"
git_new = b"p_.pinBaseRootsAfterGrantProjection(),void           0,p_.setStoreDeniedDomains(TP())"
custom_mcp_old = b"let p_=Date.now();return T2"
custom_mcp_new = b"let p_=Date.now();return;T2"
conda_git_old = b"let G=await this._ensureGitScan(),W=rL.conda"
conda_git_core = b"let G=XV9,W=rL.conda"
conda_git_new = conda_git_core + (b" " * (len(conda_git_old) - len(conda_git_core)))
conda_profile_old = (
    b'userGrantsVisible:!0,condaHomeVisibleInConfig:!0,'
    b'resetAllowWriteToDefaults:!0,workspaceDenyWriteCarveOuts:!1,'
    b'workspaceTmpContentHide:!0,denyLinkCreation:!1,network:"inherit",'
    b'gpuPassthrough:!1,writablePaths:"condaHome"'
)
conda_profile_new = conda_profile_old.replace(
    b"userGrantsVisible:!0", b"userGrantsVisible:!1", 1
)

if len(old) != 136 or len(new_core) >= len(old) or len(new) != len(old):
    raise SystemExit("lazy MCP patch must remain an equal-length 136-byte replacement")
if data.count(old) != 1:
    raise SystemExit(f"lazy MCP original byte pattern must be unique; found {data.count(old)}")
if data.count(new) != 0:
    raise SystemExit(f"locked vendor binary must remain pristine; patched byte pattern count is {data.count(new)}")
if len(git_old) != 85 or len(git_new) != 85:
    raise SystemExit("lazy Git scan patch must remain an equal-length 85-byte replacement")
if data.count(git_old) != 1:
    raise SystemExit(f"eager Git warmup byte pattern must be unique; found {data.count(git_old)}")
if data.count(git_new) != 0:
    raise SystemExit(f"locked vendor binary must remain pristine; lazy Git pattern count is {data.count(git_new)}")
if len(custom_mcp_old) != 27 or len(custom_mcp_new) != 27:
    raise SystemExit("custom MCP warmup patch must remain an equal-length 27-byte replacement")
if data.count(custom_mcp_old) != 1 or data.count(custom_mcp_new) != 0:
    raise SystemExit("locked vendor binary custom MCP boot warmup identity changed")
if len(conda_git_old) != 44 or len(conda_git_new) != 44:
    raise SystemExit("conda Git-scan patch must remain an equal-length 44-byte replacement")
if data.count(conda_git_old) != 1 or data.count(conda_git_new) != 0:
    raise SystemExit("locked vendor binary conda Git-scan identity changed")
if len(conda_profile_old) != 217 or len(conda_profile_new) != 217:
    raise SystemExit("conda profile patch must remain an equal-length 217-byte replacement")
if data.count(conda_profile_old) != 1 or data.count(conda_profile_new) != 0:
    raise SystemExit("locked vendor binary conda profile identity changed")

markers = {
    b"._loadBundledServer(": 2,
    b"_loadBundledServer(": 3,
    b"snapshotFor(z,O){return this._assemble(z,O,{waitBudgetMs:0})": 1,
    b"G.serverName===M": 1,
    b".ready(w,z.db,{serverName:": 1,
    b'x_.ops.agents.reseedOperonProfile("local-dev").then': 1,
    b"[buildApp] custom-MCP warmup complete": 1,
    b"async ensureMcpEnv(z)": 1,
    b"mcpEnvFirst": 2,
    b"if(G?.mcpEnvFirst)try{await w.ensureMcpEnv({})": 1,
    b"async wrapCondaCommand(z,O)": 1,
    b"let G=await this._ensureGitScan(),W=rL.conda": 1,
    b"XV9={bareArtifactConjunct:!1,gitDirs:[],pointerFiles:[]": 1,
    b"z.gitScan??w.scanGitStructures": 1,
    b'if(W==="rw"&&!G?.reassert)O.warmGitScan?.().catch(()=>{})': 1,
    b"async _ensureGitScan()": 1,
    b"await this._ensureGitScan()": 4,
    b"if(this._gitRescanInFlight===null&&!this._gitRescanDisabled)this._kickGitRescan();": 1,
    b"async warmGitScan()": 1,
    b"GIT_SCAN_REFRESH_MS=1e4": 1,
    b"GIT_SCAN_IDLE_QUIESCE=6": 1,
    b"warmGitScan:()=>p_.warmGitScan()": 1,
}
for marker, expected in markers.items():
    actual = data.count(marker)
    if actual != expected:
        raise SystemExit(
            f"lazy MCP call-path marker {marker!r} expected {expected}, found {actual}"
        )
'@
  & $Python -c $LazyMcpBinaryCheck $RuntimeBinaryPath
  if ($LASTEXITCODE -ne 0) {
    throw "Bundled Claude Science lazy-MCP binary policy failed (exit $LASTEXITCODE)."
  }
}
$StartScriptText = Get-Content -LiteralPath $StartScriptPath -Raw -Encoding UTF8
if (-not $StartScriptText.Contains("--no-auto-update")) {
  throw "Managed Claude Science must disable upstream background auto-update."
}
if (-not $StartScriptText.Contains('if [ "${CSA_BRIDGE_ONLY:-0}" = "1" ]')) {
  throw "Provider switching must support a Bridge-only restart mode."
}
$BridgeOnlyPosition = $StartScriptText.IndexOf('if [ "${CSA_BRIDGE_ONLY:-0}" = "1" ]')
$FullActivationGuardPosition = $StartScriptText.IndexOf('if [ "${CSA_BRIDGE_ONLY:-0}" != "1" ]; then')
$ClaudeStopPosition = $StartScriptText.IndexOf('stop_existing_claude_for_activation || exit 1')
$BridgeStagePosition = $StartScriptText.IndexOf('csa_stage_bridge_runtime "$PROJECT_DIR"')
if (
  $FullActivationGuardPosition -lt 0 -or
  $ClaudeStopPosition -lt $FullActivationGuardPosition -or
  $BridgeStagePosition -lt $ClaudeStopPosition
) {
  throw "Full activation must stop Claude Science before Bridge staging, while Bridge-only mode skips that stop."
}
$BridgeOnlyTail = $StartScriptText.Substring($BridgeOnlyPosition, [Math]::Min(400, $StartScriptText.Length - $BridgeOnlyPosition))
if (-not $BridgeOnlyTail.Contains('START_COMPLETED=1') -or -not $BridgeOnlyTail.Contains('exit 0')) {
  throw "Bridge-only restart must exit before Claude Science runtime staging."
}
Write-Host "Claude Science 0.1.25 runtime lock checks passed"

$RuntimeUpdateSource = Join-Path $ProjectDir "launcher\src\runtimeUpdate.ts"
$LauncherRustSource = Join-Path $ProjectDir "launcher\src-tauri\src\lib.rs"
if (-not (Test-Path -LiteralPath $RuntimeUpdateSource)) {
  throw "Runtime update Prompt generator is missing."
}
$RuntimeUpdateText = Get-Content -LiteralPath $RuntimeUpdateSource -Raw -Encoding UTF8
foreach ($pattern in @('invoke\s*\(', 'fetch\s*\(', 'child_process', 'execFile\s*\(', 'writeFile\s*\(')) {
  if ($RuntimeUpdateText -match $pattern) {
    throw "Runtime update Prompt generator must remain side-effect free; forbidden pattern: $pattern"
  }
}
foreach ($marker in @('0.1.21', '0.1.25', 'SQLite backup API', 'wsl --unregister', 'SHA-256')) {
  if (-not $RuntimeUpdateText.Contains($marker)) {
    throw "Runtime update Prompt is missing required safety marker: $marker"
  }
}
$LauncherRustText = Get-Content -LiteralPath $LauncherRustSource -Raw -Encoding UTF8
foreach ($marker in @('get_runtime_update_status', 'CLAUDE_SCIENCE_RELEASE_BASE', 'BUNDLED_CLAUDE_SCIENCE_VERSION')) {
  if (-not $LauncherRustText.Contains($marker)) {
    throw "Runtime update checker is missing required marker: $marker"
  }
}
Write-Host "runtime update checker and Prompt safety checks passed"

$MigrationPromptSource = Join-Path $ProjectDir "launcher\src\storageMigration.ts"
$MigrationPromptDocument = Join-Path $ProjectDir "docs\prompts\csa-wsl-storage-migration-codex-prompt.zh-CN.md"
if (Test-Path -LiteralPath $MigrationPromptSource) {
  $MigrationPromptText = Get-Content -LiteralPath $MigrationPromptSource -Raw -Encoding UTF8
  foreach ($pattern in @('invoke\s*\(', 'fetch\s*\(', 'child_process', 'execFile\s*\(', 'writeFile\s*\(')) {
    if ($MigrationPromptText -match $pattern) {
      throw "Storage migration Prompt generator must remain side-effect free; forbidden pattern: $pattern"
    }
  }
} elseif (Test-Path -LiteralPath $MigrationPromptDocument) {
  $MigrationPromptText = Get-Content -LiteralPath $MigrationPromptDocument -Raw -Encoding UTF8
} else {
  throw "Storage migration Prompt generator/document is missing."
}
foreach ($marker in @('BUILD NO-GO', 'wsl --unregister', 'source_path', 'config revision')) {
  if (-not $MigrationPromptText.Contains($marker)) {
    throw "Storage migration Prompt is missing required safety marker: $marker"
  }
}
Write-Host "storage migration Prompt safety checks passed"

$code = @'
import importlib.util
from pathlib import Path

path = Path("tests/test_translation.py")
spec = importlib.util.spec_from_file_location("test_translation", path)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
tests = sorted(name for name in dir(mod) if name.startswith("test_"))
for name in tests:
    getattr(mod, name)()
    print(f"{name} passed")
print(f"{len(tests)} translation tests passed")
'@
$tmp = New-TemporaryFile
try {
  Set-Content -LiteralPath $tmp -Value $code -Encoding UTF8
  & $Python $tmp
  if ($LASTEXITCODE -ne 0) { throw "Translation tests failed (exit $LASTEXITCODE)." }
} finally {
  Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
}

& (Join-Path $ProjectDir "tests\package_policy_test.ps1")
if ($LASTEXITCODE -ne 0) { throw "Package policy tests failed (exit $LASTEXITCODE)." }

& $Python -m pytest tests/test_network_quality.py tests/test_narrow_broad_host_grants.py -q
if ($LASTEXITCODE -ne 0) { throw "Network quality and host-grant safety tests failed (exit $LASTEXITCODE)." }

Write-Host "self-test passed"
