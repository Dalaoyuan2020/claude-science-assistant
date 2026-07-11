#!/usr/bin/env bash
set -euo pipefail

json_bool() {
  if "$@" >/dev/null 2>&1; then printf 'true'; else printf 'false'; fi
}

json_string() {
  local value
  value="$(cat)"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  value="${value//$'\r'/\\r}"
  printf '"%s"' "$value"
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
bridge_health_responding=false
bridge_source_matches=null
bridge_source_path=""
health_payload="$(curl -fsS --connect-timeout 0.4 --max-time 1 http://127.0.0.1:9876/health 2>/dev/null || true)"
if [ -n "$health_payload" ]; then
  bridge_health_responding=true
  health_python=""
  if [ -x "$venv_python" ]; then
    health_python="$venv_python"
  elif command -v python3 >/dev/null 2>&1; then
    health_python="$(command -v python3)"
  fi
  health_state="foreign"
  if [ -n "$health_python" ]; then
    bridge_source_path="$("$health_python" -c '
import json, sys
try:
    health = json.loads(sys.argv[1])
except Exception:
    raise SystemExit
print(str(health.get("source_path") or ""))
' "$health_payload" 2>/dev/null || true)"
    health_state="$("$health_python" -c '
import json, os, re, sys
try:
    health = json.loads(sys.argv[2])
except Exception:
    print("invalid")
    raise SystemExit
expected = os.path.realpath(sys.argv[1])
actual = os.path.realpath(str(health.get("source_path") or ""))

def comparison_key(path):
    # DrvFs paths inherit Windows case-insensitive path identity even though
    # Python is running inside Linux. Native Linux paths remain case-sensitive.
    return path.casefold() if re.match(r"^/mnt/[a-zA-Z]/", path) else path

print(
    "current"
    if health.get("status") == "ok"
    and comparison_key(actual) == comparison_key(expected)
    else "foreign"
)
' "$project_dir/proxy.py" "$health_payload" 2>/dev/null || true)"
  fi
  if [ "$health_state" = current ]; then
    bridge_healthy=true
    bridge_source_matches=true
  else
    bridge_source_matches=false
  fi
fi
tmp_writable=false
tmp_dir="${TMPDIR:-/tmp}"
if [ -d "$tmp_dir" ] && [ -w "$tmp_dir" ] \
  && ! findmnt -no OPTIONS -T "$tmp_dir" 2>/dev/null | tr ',' '\n' | grep -qx ro; then
  tmp_writable=true
fi
home_writable=false
if [ -d "$HOME" ] && [ -w "$HOME" ] \
  && ! findmnt -no OPTIONS -T "$HOME" 2>/dev/null | tr ',' '\n' | grep -qx ro; then
  home_writable=true
fi
root_total_kb="$(LC_ALL=C df -Pk / 2>/dev/null | awk 'NR==2 {print $2}' || true)"
root_free_kb="$(LC_ALL=C df -Pk / 2>/dev/null | awk 'NR==2 {print $4}' || true)"
root_inode_total="$(LC_ALL=C df -Pi / 2>/dev/null | awk 'NR==2 {print $2}' || true)"
root_inode_free="$(LC_ALL=C df -Pi / 2>/dev/null | awk 'NR==2 {print $4}' || true)"
root_options="$(findmnt -no OPTIONS -T / 2>/dev/null || true)"
root_read_only=false
if printf '%s\n' "$root_options" | tr ',' '\n' | grep -qx ro; then root_read_only=true; fi
bridge_log_bytes="$(stat -c %s "$HOME/.claude-science/logs/wsl-proxy.log" 2>/dev/null || true)"
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
printf '"bridge_venv":%s,' "$(json_bool test -x "$venv_python")"
printf '"tmp_writable":%s,' "$tmp_writable"
printf '"home_writable":%s' "$home_writable"
printf '},'
printf '"storage":{'
printf '"root_total_kb":%s,' "${root_total_kb:-null}"
printf '"root_free_kb":%s,' "${root_free_kb:-null}"
printf '"root_inode_total":%s,' "${root_inode_total:-null}"
printf '"root_inode_free":%s,' "${root_inode_free:-null}"
printf '"root_read_only":%s,' "$root_read_only"
printf '"bridge_log_bytes":%s' "${bridge_log_bytes:-null}"
printf '},'
printf '"runtime":{'
printf '"bridge_pid":%s,' "${bridge_pid:-null}"
printf '"claude_pid":%s,' "${claude_pid:-null}"
printf '"bridge_healthy":%s,' "$bridge_healthy"
printf '"bridge_health_responding":%s,' "$bridge_health_responding"
printf '"bridge_source_path":%s,' "$(printf '%s' "$bridge_source_path" | json_string)"
printf '"bridge_source_matches":%s,' "$bridge_source_matches"
printf '"bridge_service_active":%s,' "$bridge_service_active"
printf '"unit_matches_project":%s,' "$unit_matches_project"
printf '"port_9876":%s,' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":9876 "')"
printf '"port_8765":%s,' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":8765 "')"
printf '"port_8766":%s' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":8766 "')"
printf '},'
printf '"secrets":{"values_included":false}'
printf '}\n'
