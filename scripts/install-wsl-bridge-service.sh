#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROXY_PORT="${PROXY_PORT:-9876}"
SERVICE_NAME="claude-science-bridge.service"
UNIT_DIR="$HOME/.config/systemd/user"
UNIT_PATH="$UNIT_DIR/$SERVICE_NAME"
CONFIG_DIR="$HOME/.claude-science/proxy"

if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" != "systemd" ]; then
  echo "systemd is not PID 1; user service installation is unavailable." >&2
  exit 2
fi

if [ -x "$HOME/.local/share/claude-science-api-bridge/venv/bin/python" ]; then
  PYTHON_BIN="${PYTHON:-$HOME/.local/share/claude-science-api-bridge/venv/bin/python}"
else
  PYTHON_BIN="${PYTHON:-$(command -v python3)}"
fi

unit_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

mkdir -p "$UNIT_DIR" "$HOME/.claude-science/logs" "$CONFIG_DIR"
if [ ! -f "$CONFIG_DIR/config.json" ] && [ -f "$PROJECT_DIR/config.json" ]; then
  cp -f "$PROJECT_DIR/config.json" "$CONFIG_DIR/config.json"
  chmod 600 "$CONFIG_DIR/config.json"
fi
python_escaped="$(unit_escape "$PYTHON_BIN")"
proxy_escaped="$(unit_escape "$PROJECT_DIR/proxy.py")"
config_dir_escaped="$(unit_escape "$CONFIG_DIR")"
static_dir_escaped="$(unit_escape "$PROJECT_DIR/static")"

tmp="$(mktemp "$UNIT_DIR/.${SERVICE_NAME}.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
cat >"$tmp" <<EOF
[Unit]
Description=Claude Science Assistant API Bridge
After=network-online.target

[Service]
Type=simple
Environment="PROXY_HOST=127.0.0.1"
Environment="PROXY_PORT=$PROXY_PORT"
Environment="CLAUDE_SCIENCE_PROXY_DIR=$config_dir_escaped"
Environment="CLAUDE_SCIENCE_STATIC_DIR=$static_dir_escaped"
ExecStart="$python_escaped" "$proxy_escaped"
Restart=on-failure
RestartSec=2
StandardOutput=append:%h/.claude-science/logs/wsl-proxy.log
StandardError=append:%h/.claude-science/logs/wsl-proxy.log

[Install]
WantedBy=default.target
EOF

chmod 600 "$tmp"
mv -f "$tmp" "$UNIT_PATH"
trap - EXIT
systemctl --user daemon-reload
systemctl --user enable "$SERVICE_NAME" >/dev/null
echo "$UNIT_PATH"
