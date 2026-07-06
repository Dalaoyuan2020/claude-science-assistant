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
  $python = Get-Command python -ErrorAction SilentlyContinue
  if ($py) {
    & $py.Source -3 -m venv $venvDir
  } elseif ($python) {
    & $python.Source -m venv $venvDir
  } else {
    throw "Python not found. Install Python 3 or set PYTHON to python.exe."
  }
  $Python = (Resolve-Path -LiteralPath $defaultPython).Path
  & $Python -m pip install --upgrade pip
  & $Python -m pip install -r (Join-Path $ProjectDir "requirements.txt")
}

& $Python -m py_compile proxy.py setup-token.py forward-443.py

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
} finally {
  Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
}

Write-Host "self-test passed"
