#!/usr/bin/env bash
set -euo pipefail

json_bool() {
  if "$@" >/dev/null 2>&1; then printf 'true'; else printf 'false'; fi
}

json_string() {
  python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))'
}

user_name="$(id -un)"
distro="${WSL_DISTRO_NAME:-unknown}"
project_dir="${1:-${PROJECT_DIR:-}}"
systemd_running=false
if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" = "systemd" ]; then systemd_running=true; fi

source_bin="$HOME/.local/bin/claude-science"
managed_bin="$HOME/.local/share/claude-science-api-bridge/bin/claude-science"
patched_bin="$HOME/.local/share/claude-science-api-bridge/patched/claude-science"
venv_python="$HOME/.local/share/claude-science-api-bridge/venv/bin/python"
bridge_pid="$(ps -eo pid=,args= | awk '/python/ && /proxy.py/ && !/awk/ {print $1; exit}' || true)"
claude_pid="$(ps -eo pid=,args= | awk '/claude-science/ && /serve/ && !/awk/ {print $1; exit}' || true)"
bridge_healthy=false
if curl -fsS --max-time 2 http://127.0.0.1:9876/health >/dev/null 2>&1; then bridge_healthy=true; fi
bridge_service_active=false
unit_matches_project=null
if [ "$systemd_running" = true ]; then
  if systemctl --user is-active --quiet claude-science-bridge.service; then bridge_service_active=true; fi
  if [ -n "$project_dir" ]; then
    if systemctl --user cat claude-science-bridge.service 2>/dev/null | grep -F -- "$project_dir/proxy.py" >/dev/null 2>&1; then
      unit_matches_project=true
    else
      unit_matches_project=false
    fi
  fi
fi

printf '{'
printf '"schema_version":1,'
printf '"generated_at":%s,' "$(date -u +%Y-%m-%dT%H:%M:%SZ | json_string)"
printf '"mode":"read-only",'
printf '"wsl":{'
printf '"distro":%s,' "$(printf '%s' "$distro" | json_string)"
printf '"user":%s,' "$(printf '%s' "$user_name" | json_string)"
printf '"kernel":%s,' "$(uname -r | json_string)"
printf '"systemd":%s' "$systemd_running"
printf '},'
printf '"components":{'
printf '"python3":%s,' "$(json_bool command -v python3)"
printf '"curl":%s,' "$(json_bool command -v curl)"
printf '"source_binary":%s,' "$(json_bool bash -c "test -x '$managed_bin' || test -x '$source_bin'")"
printf '"managed_binary":%s,' "$(json_bool test -x "$managed_bin")"
printf '"user_binary":%s,' "$(json_bool test -x "$source_bin")"
printf '"patched_binary":%s,' "$(json_bool test -x "$patched_bin")"
printf '"bridge_venv":%s' "$(json_bool test -x "$venv_python")"
printf '},'
printf '"runtime":{'
printf '"bridge_pid":%s,' "${bridge_pid:-null}"
printf '"claude_pid":%s,' "${claude_pid:-null}"
printf '"bridge_healthy":%s,' "$bridge_healthy"
printf '"bridge_service_active":%s,' "$bridge_service_active"
printf '"unit_matches_project":%s,' "$unit_matches_project"
printf '"port_9876":%s,' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":9876 "')"
printf '"port_8765":%s,' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":8765 "')"
printf '"port_8766":%s' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":8766 "')"
printf '},'
printf '"secrets":{"values_included":false}'
printf '}\n'
