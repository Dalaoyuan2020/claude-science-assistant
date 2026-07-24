param(
  [string]$Python = $(if ($env:PYTHON) { $env:PYTHON } else { ".\.venv\Scripts\python.exe" })
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Resolve-Path (Join-Path $ScriptDir "..")
Set-Location $ProjectDir

if (-not (Test-Path $Python)) {
  $defaultPython = ".\.venv\Scripts\python.exe"
  if ($Python -ne $defaultPython -or $env:PYTHON) {
    throw "Python not found: $Python"
  }

  $venvDir = Join-Path $ProjectDir ".venv"
  $py = Get-Command py -ErrorAction SilentlyContinue
  $pythonCommand = Get-Command python -ErrorAction SilentlyContinue
  if ($py) {
    & $py.Source -3 -m venv $venvDir
  } elseif ($pythonCommand) {
    & $pythonCommand.Source -m venv $venvDir
  } else {
    throw "Python not found. Install Python 3 or set PYTHON to python.exe."
  }
  if ($LASTEXITCODE -ne 0) { throw "Failed to create Python virtual environment (exit $LASTEXITCODE)." }
  $Python = $defaultPython
}

$Python = (Resolve-Path -LiteralPath $Python).Path
$previousErrorAction = $ErrorActionPreference
$ErrorActionPreference = "SilentlyContinue"
try {
  & $Python -c "import fastapi, httpx, starlette, uvicorn" 2>$null
  $dependencyProbeExit = $LASTEXITCODE
} finally {
  $ErrorActionPreference = $previousErrorAction
}
$dependenciesReady = ($dependencyProbeExit -eq 0)
if (-not $dependenciesReady) {
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

& $Python -m py_compile proxy.py setup-token.py forward-443.py
if ($LASTEXITCODE -ne 0) { throw "Python syntax check failed (exit $LASTEXITCODE)." }

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

Write-Host "self-test passed"
