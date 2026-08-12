# CSA Claude Science 启动脚本 (Git Bash / MSYS)
# 用法: bash start.sh [--no-open]
# 启动 CSA proxy bridge + WSL Claude Science daemon

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== CSA Claude Science 启动 ==="
echo ""

# 1) 启动 Windows 侧 CSA Bridge (proxy.py on 127.0.0.1:9876)
echo "[1/3] 启动 CSA Bridge (127.0.0.1:9876)..."
"$SCRIPT_DIR/run-proxy.sh" &
BRIDGE_PID=$!
sleep 3

# 2) 检查 bridge 健康
if curl -sf http://127.0.0.1:9876/health >/dev/null 2>&1; then
  echo "      Bridge 已就绪 (pid=$BRIDGE_PID)"
else
  echo "      Bridge 启动中..."
  sleep 3
fi

# 3) 启动 WSL Claude Science
echo ""
echo "[2/3] 启动 WSL Claude Science..."
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$SCRIPT_DIR/scripts/start-claude-science-wsl.ps1" -Open

echo ""
echo "[3/3] 完成"
echo "Dashboard: http://127.0.0.1:9876/dashboard"
