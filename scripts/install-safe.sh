#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$SCRIPT_DIR/install-safe.ps1" "$@"
