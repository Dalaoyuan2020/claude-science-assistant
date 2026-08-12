# CSA Claude Science 停止脚本
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== CSA Claude Science 停止 ==="

# 停止 WSL Claude Science (自动检测默认 WSL 发行版)
WSL_DISTRO=$(powershell.exe -NoProfile -Command "wsl --list --quiet 2>null | Select-Object -First 1" 2>/dev/null | tr -d '\r' || echo "Ubuntu")
if [ -n "$WSL_DISTRO" ]; then
  powershell.exe -NoProfile -Command "wsl -d $WSL_DISTRO -- bash -lc 'pkill -f \"claude-science serve\" 2>/dev/null || true'" 2>/dev/null || true
fi

# 停止 bridge
pkill -f "proxy.py" 2>/dev/null || true

echo "已停止"
