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
network_helper="$project_dir/scripts/csa-network-quality.py"
systemd_running=false
if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" = "systemd" ]; then systemd_running=true; fi

state_root="${CSA_STATE_ROOT:-$HOME/.local/share/csa}"
network_cache_file="$state_root/runtime/network-quality.json"
legacy_state_root="$HOME/.local/share/claude-science-api-bridge"
source_bin="$HOME/.local/bin/claude-science"
managed_bin="$state_root/runtime/claude-science/current/claude-science"
legacy_managed_bin="$legacy_state_root/bin/claude-science"
patched_bin="$state_root/runtime/claude-science/patched-current/claude-science"
venv_python="$legacy_state_root/venv/bin/python"
bridge_current="$state_root/runtime/bridge/current"
bridge_proxy="$bridge_current/proxy.py"
listener_pids() {
  local port="$1"
  ss -ltnp "sport = :$port" 2>/dev/null \
    | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u
}
bridge_pid_list="$(listener_pids 9876 || true)"
bridge_pid=""
if [ "$(printf '%s\n' "$bridge_pid_list" | sed '/^$/d' | wc -l)" = "1" ]; then
  bridge_pid="$(printf '%s\n' "$bridge_pid_list" | sed '/^$/d' | head -1)"
fi
claude_primary_pid_list="$(listener_pids 8765 || true)"
claude_auxiliary_pid_list="$(listener_pids 8766 || true)"
claude_listener_pid_list="$(printf '%s\n%s\n' "$claude_primary_pid_list" "$claude_auxiliary_pid_list" | sed '/^$/d' | sort -u)"
claude_pid=""
claude_unverified_pid=""
claude_owner_verified=false
if [ "$(printf '%s\n' "$claude_primary_pid_list" | sed '/^$/d' | wc -l)" = "1" ] \
  && [ "$(printf '%s\n' "$claude_auxiliary_pid_list" | sed '/^$/d' | wc -l)" = "1" ] \
  && [ "$claude_primary_pid_list" = "$claude_auxiliary_pid_list" ]; then
  candidate_pid="$claude_primary_pid_list"
  candidate_executable="$(readlink -f "/proc/$candidate_pid/exe" 2>/dev/null || true)"
  candidate_command="$(tr '\0' ' ' <"/proc/$candidate_pid/cmdline" 2>/dev/null || true)"
  case "$candidate_executable" in
    "$state_root"/runtime/claude-science/patched/*/claude-science|"$legacy_state_root"/patched/claude-science)
      if [[ "$candidate_command" == *"$candidate_executable serve"* ]]; then
        claude_pid="$candidate_pid"
        claude_owner_verified=true
      fi
      ;;
  esac
  if [ "$claude_owner_verified" != true ]; then
    claude_unverified_pid="$candidate_pid"
  fi
elif [ -n "$claude_listener_pid_list" ]; then
  claude_unverified_pid="$(printf '%s\n' "$claude_listener_pid_list" | sed '/^$/d' | head -1)"
fi
network_fallback_json() {
  local fallback_pid="${1:-null}"
  local fallback_proxy_state="${2:-unknown}"
  local fallback_egress_state="${3:-not_checked}"
  printf '%s' \
    '{"schema_version":3,' \
    "\"claude_pid\":$fallback_pid," \
    '"claude_start_ticks":null,' \
    '"claude_process_state":"unknown",' \
    '"claude_wait_channel":"unknown",' \
    '"claude_io_blocked":false,' \
    '"claude_mount_io_blocked":false,' \
    "\"proxy_state\":\"$fallback_proxy_state\"," \
    '"proxy_reachable":null,' \
    '"proxy_endpoints":[],' \
    '"proxy_variable_names":[],' \
    '"proxy_conflict":false,' \
    '"deep_checked":false,' \
    '"deep_checked_at_unix":null,' \
    '"sandbox_contract_stable_during_probe":false,' \
    '"sandbox_forwarder_count":0,' \
    '"sandbox_forwarder_group_count":0,' \
    '"sandbox_http_forwarder_count":0,' \
    '"sandbox_socks_forwarder_count":0,' \
    '"sandbox_forwarder_expected_count":3,' \
    '"sandbox_forwarder_topology_state":"incomplete",' \
    '"sandbox_forwarder_incomplete_extra_count":0,' \
    '"sandbox_forwarder_fingerprint":null,' \
    '"sandbox_probe_identity":"analysis-socks5h-pypi-head-v2",' \
    '"sandbox_probe_role":"analysis",' \
    '"sandbox_probe_transport":"socks5h",' \
    '"sandbox_unix_socket_state":"not_checked",' \
    '"sandbox_socks_handshake_state":"not_checked",' \
    '"sandbox_socks_handshake_error":null,' \
    "\"sandbox_egress_state\":\"$fallback_egress_state\"," \
    '"sandbox_egress_failure_stage":"not_checked",' \
    '"sandbox_egress_target":"pypi.org",' \
    '"sandbox_egress_canary_identity":"https://pypi.org/simple/pip/",' \
    '"sandbox_egress_canary_fingerprint":null,' \
    '"sandbox_egress_http_status":null,' \
    '"sandbox_egress_http_statuses":[],' \
    '"sandbox_forwarder_probe_count":0,' \
    '"sandbox_forwarder_passed_count":0,' \
    '"sandbox_forwarder_failed_count":0,' \
    '"sandbox_egress_curl_exit_code":null,' \
    '"sandbox_probe_daemon_state":"unknown",' \
    '"sandbox_probe_daemon_wait_channel":"unknown",' \
    '"sandbox_probe_daemon_io_blocked":false,' \
    '"sandbox_probe_daemon_mount_io_blocked":false,' \
    '"secrets_included":false}'
}
network_json="$(network_fallback_json null not_running not_checked)"
network_python=""
if [ -x "$venv_python" ]; then
  network_python="$venv_python"
elif command -v python3 >/dev/null 2>&1; then
  network_python="$(command -v python3)"
fi
if [ -n "$claude_pid" ]; then
  if [ -n "$network_python" ] && [ -f "$network_helper" ]; then
    network_args=(--pid "$claude_pid" --cache-file "$network_cache_file")
    if [ "${CSA_DEEP_NETWORK_PROBE:-0}" = "1" ]; then
      network_args+=(--deep)
      if [ "${CSA_WRITE_NETWORK_CACHE:-0}" = "1" ]; then
        network_args+=(--write-cache)
      fi
    fi
    network_result="$($network_python "$network_helper" "${network_args[@]}" 2>/dev/null || true)"
    if [[ "$network_result" == \{*\} ]]; then
      network_json="$network_result"
    else
      network_json="$(network_fallback_json "$claude_pid" unknown unavailable)"
    fi
  else
    network_json="$(network_fallback_json "$claude_pid" unknown unavailable)"
  fi
fi
bridge_healthy=false
bridge_health_responding=false
bridge_source_matches=null
bridge_source_path=""
bridge_identity_json=null
health_payload="$(curl --noproxy '*' -fsS --connect-timeout 0.4 --max-time 1 http://127.0.0.1:9876/health 2>/dev/null || true)"
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
    bridge_identity_json="$("$health_python" -c '
import json, sys
try:
    health = json.loads(sys.argv[1])
except Exception:
    print("null")
    raise SystemExit
identity = health.get("runtime_identity")
print(json.dumps(identity, separators=(",", ":")) if isinstance(identity, dict) else "null")
' "$health_payload" 2>/dev/null || printf null)"
    health_state="$("$health_python" -c '
import hashlib, json, os, re, sys
try:
    health = json.loads(sys.argv[2])
    listener_pid = int(sys.argv[3])
except Exception:
    print("invalid")
    raise SystemExit
expected = os.path.realpath(sys.argv[1])
actual = os.path.realpath(str(health.get("source_path") or ""))
identity = health.get("runtime_identity") or {}
runtime_dir = os.path.dirname(expected)
manifest_path = os.path.join(runtime_dir, "runtime-manifest.json")

try:
    with open(manifest_path, encoding="utf-8") as stream:
        manifest = json.load(stream)
    source_sha = hashlib.sha256(open(expected, "rb").read()).hexdigest()
    bundle_lines = []
    for relative in ("proxy.py", "setup-token.py", "requirements.txt", "static/dashboard.html"):
        path = os.path.join(runtime_dir, *relative.split("/"))
        digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
        bundle_lines.append(f"{digest}  {relative}\n")
    bundle_sha = hashlib.sha256("".join(bundle_lines).encode()).hexdigest()
except (OSError, ValueError, TypeError):
    manifest = {}
    source_sha = ""
    bundle_sha = ""

def comparison_key(path):
    # DrvFs paths inherit Windows case-insensitive path identity even though
    # Python is running inside Linux. Native Linux paths remain case-sensitive.
    return path.casefold() if re.match(r"^/mnt/[a-zA-Z]/", path) else path

print(
    "current"
    if health.get("status") == "ok"
    and comparison_key(actual) == comparison_key(expected)
    and identity.get("schemaVersion") == 1
    and identity.get("component") == "bridge"
    and identity.get("managed") is True
    and manifest.get("schemaVersion") == 1
    and manifest.get("component") == "bridge"
    and identity.get("runtimeId") == manifest.get("runtimeId")
    and identity.get("version") == manifest.get("version")
    and identity.get("sourceSha256", "").casefold() == source_sha
    and manifest.get("sourceSha256", "").casefold() == source_sha
    and manifest.get("bundleSha256", "").casefold() == bundle_sha
    and identity.get("runtimeId") == "bridge-{}-{}".format(manifest.get("version"), bundle_sha[:16])
    and os.path.basename(runtime_dir) == identity.get("runtimeId")
    and comparison_key(os.path.realpath(str(identity.get("sourcePath") or ""))) == comparison_key(expected)
    and isinstance(identity.get("runtimeId"), str)
    and bool(identity.get("runtimeId"))
    and isinstance(identity.get("buildId"), str)
    and identity.get("buildId", "").casefold() == source_sha[:16]
    and isinstance(identity.get("sourceSha256"), str)
    and bool(re.fullmatch(r"[0-9a-fA-F]{64}", identity.get("sourceSha256")))
    and isinstance(identity.get("pid"), int)
    and identity.get("pid") == listener_pid
    and "health" in (identity.get("capabilities") or [])
    else "foreign"
)
' "$bridge_proxy" "$health_payload" "$bridge_pid" 2>/dev/null || true)"
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
  if [ "$bridge_healthy" = true ] && [ "$bridge_identity_json" != null ]; then
    unit_runtime_id="$("$health_python" -c 'import json,sys; print(json.loads(sys.argv[1]).get("runtimeId") or "")' "$bridge_identity_json" 2>/dev/null || true)"
    unit_source_sha="$("$health_python" -c 'import json,sys; print(json.loads(sys.argv[1]).get("sourceSha256") or "")' "$bridge_identity_json" 2>/dev/null || true)"
    unit_text="$(systemctl --user cat claude-science-bridge.service 2>/dev/null || true)"
    if [ -n "$unit_runtime_id" ] && [ -n "$unit_source_sha" ] \
      && grep -F -- "$bridge_proxy" <<<"$unit_text" >/dev/null 2>&1 \
      && grep -F -- "CSA_BRIDGE_RUNTIME_ID=$unit_runtime_id" <<<"$unit_text" >/dev/null 2>&1 \
      && grep -F -- "CSA_BRIDGE_SOURCE_SHA256=$unit_source_sha" <<<"$unit_text" >/dev/null 2>&1 \
      && grep -F -- 'PROXY_PORT=9876' <<<"$unit_text" >/dev/null 2>&1 \
      && grep -F -- 'UnsetEnvironment=HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy' <<<"$unit_text" >/dev/null 2>&1; then
      unit_matches_project=true
    else
      unit_matches_project=false
    fi
  fi
fi

printf '{'
printf '"schema_version":1,'
printf '"generated_at":%s,' "$(date -u +%Y-%m-%dT%H:%M:%SZ | json_string)"
if [ "${CSA_WRITE_NETWORK_CACHE:-0}" = "1" ]; then
  printf '"mode":"diagnostic-cache",'
else
  printf '"mode":"read-only",'
fi
printf '"wsl":{'
printf '"distro":%s,' "$(printf '%s' "$distro" | json_string)"
printf '"user":%s,' "$(printf '%s' "$user_name" | json_string)"
printf '"kernel":%s,' "$(uname -r | json_string)"
printf '"systemd":%s' "$systemd_running"
printf '},'
printf '"components":{'
printf '"python3":%s,' "$(json_bool command -v python3)"
printf '"curl":%s,' "$(json_bool command -v curl)"
printf '"source_binary":%s,' "$(json_bool bash -c "test -x '$managed_bin' || test -x '$legacy_managed_bin' || test -x '$source_bin'")"
printf '"managed_binary":%s,' "$(json_bool test -x "$managed_bin")"
printf '"legacy_managed_binary":%s,' "$(json_bool test -x "$legacy_managed_bin")"
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
printf '"claude_unverified_pid":%s,' "${claude_unverified_pid:-null}"
printf '"claude_owner_verified":%s,' "$claude_owner_verified"
printf '"bridge_healthy":%s,' "$bridge_healthy"
printf '"bridge_health_responding":%s,' "$bridge_health_responding"
printf '"bridge_source_path":%s,' "$(printf '%s' "$bridge_source_path" | json_string)"
printf '"bridge_source_matches":%s,' "$bridge_source_matches"
printf '"bridge_identity":%s,' "$bridge_identity_json"
printf '"bridge_service_active":%s,' "$bridge_service_active"
printf '"unit_matches_project":%s,' "$unit_matches_project"
printf '"port_9876":%s,' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":9876 "')"
printf '"port_8765":%s,' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":8765 "')"
printf '"port_8766":%s' "$(json_bool bash -c 'ss -ltn 2>/dev/null | grep -q ":8766 "')"
printf '},'
printf '"network":%s,' "$network_json"
printf '"secrets":{"values_included":false}'
printf '}\n'
