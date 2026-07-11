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
& $Python -c "import fastapi, httpx, starlette, uvicorn"
$dependenciesReady = ($LASTEXITCODE -eq 0)
if (-not $dependenciesReady) {
  Write-Host "Python test dependencies are missing or incomplete; repairing the local venv."
  & $Python -m pip install --upgrade pip
  if ($LASTEXITCODE -ne 0) { throw "Failed to install pip (exit $LASTEXITCODE)." }
  & $Python -m pip install -r (Join-Path $ProjectDir "requirements.txt")
  if ($LASTEXITCODE -ne 0) { throw "Failed to install test requirements (exit $LASTEXITCODE)." }
  & $Python -c "import fastapi, httpx, starlette, uvicorn"
  if ($LASTEXITCODE -ne 0) { throw "Python requirements were installed but imports still fail (exit $LASTEXITCODE)." }
}

& $Python -m py_compile proxy.py setup-token.py forward-443.py
if ($LASTEXITCODE -ne 0) { throw "Python syntax check failed (exit $LASTEXITCODE)." }

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
