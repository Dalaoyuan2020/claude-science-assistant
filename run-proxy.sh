#!/usr/bin/env bash
# CSA Windows Bridge - 运行时启动脚本 (Git Bash / Windows 入口)
# 后台启动 CSA proxy.py bridge
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="$HOME/.claude-science"
LOG_DIR="$STATE_DIR/logs"
mkdir -p "$LOG_DIR"
# ANTHROPIC_BASE_URL is auto-derived from proxy config at runtime.
# If proxy_auth_mode=required, this URL must include the auth token:
#   http://127.0.0.1:9876/<your-proxy-auth-token>
# Set it explicitly below or rely on scripts/start-claude-science.ps1:
# export ANTHROPIC_BASE_URL="http://127.0.0.1:9876/cs_local_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
export PROXY_HOST="127.0.0.1"
export PROXY_PORT="9876"
cd "$SCRIPT_DIR"
echo "[CSA] Starting bridge on 127.0.0.1:9876..."
# Windows/Git Bash: use nohup so the process survives after the launcher exits
nohup "$SCRIPT_DIR/.venv/Scripts/python.exe" "$SCRIPT_DIR/proxy.py" >> "$LOG_DIR/proxy.log" 2>&1 &
disown 2>/dev/null || true
sleep 2
if curl -sf http://127.0.0.1:9876/health >/dev/null 2>&1; then
  echo "[CSA] Bridge is up (http://127.0.0.1:9876/health OK)"
else
  echo "[CSA] WARNING: bridge may still be starting — check $LOG_DIR/proxy.log"
fi
