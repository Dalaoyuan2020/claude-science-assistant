#!/usr/bin/env bash
set -euo pipefail

if [ "${CSA_MERGE_STDERR:-0}" = "1" ]; then
  exec 2>&1
fi

# Start Claude Science on Windows via WSL, using a patched Linux daemon copy.
#
# What this script does:
#   1. Starts the local BYOK proxy inside WSL on 127.0.0.1:${PROXY_PORT}.
#   2. Refreshes Claude Science's local fake OAuth token, if encryption.key exists.
#   3. Copies ~/.local/bin/claude-science to a dedicated patched copy.
#   4. Applies byte-length-preserving URL patches to the copied daemon only.
#   5. Starts Claude Science with ANTHROPIC_BASE_URL pointing at the WSL proxy.
#
# It does not modify DNS, hosts, certificates, VPN, system proxy, or port 443.
# It does not patch the original ~/.local/bin/claude-science binary.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

PROXY_PORT="${PROXY_PORT:-9876}"
CLAUDE_SCIENCE_PORT="${CLAUDE_SCIENCE_PORT:-8765}"
CSA_PACKAGE_VERSION="${CSA_PACKAGE_VERSION:-unknown}"
LEGACY_STATE_DIR="$HOME/.local/share/claude-science-api-bridge"
RUNTIME_LAYOUT_SCRIPT="$PROJECT_DIR/scripts/csa-runtime-layout.sh"
NETWORK_QUALITY_HELPER="$PROJECT_DIR/scripts/csa-network-quality.py"
if [ ! -f "$RUNTIME_LAYOUT_SCRIPT" ]; then
  echo "CSA runtime layout helper is missing: $RUNTIME_LAYOUT_SCRIPT" >&2
  exit 2
fi
if [ ! -f "$NETWORK_QUALITY_HELPER" ]; then
  echo "CSA network quality helper is missing: $NETWORK_QUALITY_HELPER" >&2
  exit 2
fi
# shellcheck source=csa-runtime-layout.sh
source "$RUNTIME_LAYOUT_SCRIPT"
NETWORK_CACHE_FILE="$CSA_STATE_ROOT/runtime/network-quality.json"
BRIDGE_PROXY="$CSA_BRIDGE_ROOT/current/proxy.py"
BRIDGE_STATIC="$CSA_BRIDGE_ROOT/current/static"
PATCH_DIR_OVERRIDE="${PATCH_DIR:-}"
LOG_DIR="$HOME/.claude-science/logs"
LOG_FILE="$LOG_DIR/wsl-proxy.log"

if [ -x "$LEGACY_STATE_DIR/venv/bin/python" ]; then
  PYTHON_BIN="${PYTHON:-$LEGACY_STATE_DIR/venv/bin/python}"
else
  PYTHON_BIN="${PYTHON:-python3}"
fi

current_proxy_state() {
  "$PYTHON_BIN" "$NETWORK_QUALITY_HELPER" --current --state-only 2>/dev/null \
    || printf 'unknown\n'
}

listener_pids_for_port() {
  local port="$1"
  ss -ltnp "sport = :$port" 2>/dev/null \
    | grep -o 'pid=[0-9]*' \
    | cut -d= -f2 \
    | sort -u
}

claude_primary_pid() {
  local primary_pids auxiliary_pids primary_pid auxiliary_pid
  primary_pids="$(listener_pids_for_port "$CLAUDE_SCIENCE_PORT" || true)"
  auxiliary_pids="$(listener_pids_for_port 8766 || true)"
  [ "$(printf '%s\n' "$primary_pids" | sed '/^$/d' | wc -l)" = "1" ] || return 1
  [ "$(printf '%s\n' "$auxiliary_pids" | sed '/^$/d' | wc -l)" = "1" ] || return 1
  primary_pid="$(printf '%s\n' "$primary_pids" | sed '/^$/d' | head -1)"
  auxiliary_pid="$(printf '%s\n' "$auxiliary_pids" | sed '/^$/d' | head -1)"
  case "$primary_pid:$auxiliary_pid" in
    *[!0-9:]*) return 1;;
  esac
  [ "$primary_pid" = "$auxiliary_pid" ] || return 1
  printf '%s\n' "$primary_pid"
}

check_claude_network_contract() {
  local pid state
  pid="$(claude_primary_pid)" || return 1
  state="$("$PYTHON_BIN" "$NETWORK_QUALITY_HELPER" --pid "$pid" --contract-only 2>/dev/null \
    || printf 'unknown\n')"
  [ "$state" = "ready" ]
}

wait_claude_network_contract() {
  local timeout="${1:-5}"
  local deadline=$((SECONDS + timeout))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if check_claude_network_contract; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

network_cache_probe_token() {
  "$PYTHON_BIN" - "$1" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as stream:
        report = json.load(stream)
    values = (
        report["claude_pid"],
        report["claude_start_ticks"],
        report["sandbox_forwarder_fingerprint"],
    )
    if any(value is None or str(value).strip() == "" for value in values):
        raise ValueError("missing probe identity")
    print(":".join(str(value) for value in values))
except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError):
    raise SystemExit(1)
PY
}

record_deep_network_quality() {
  local pid verdict="unknown" attempt success_streak=0 delay
  local pending_cache="${NETWORK_CACHE_FILE}.pending.$$"
  local degraded_cache="${NETWORK_CACHE_FILE}.degraded.$$" probe_token="" success_token=""
  DEEP_NETWORK_VERDICT="unknown"
  # Never expose one transient success as the startup verdict.  Each attempt
  # writes an isolated pending report; only the second adjacent success for the
  # same PID/starttime/forwarder fingerprint is promoted atomically.
  rm -f -- "$pending_cache" "$degraded_cache" "$NETWORK_CACHE_FILE"
  for attempt in 1 2 3 4; do
    pid="$(claude_primary_pid)" || {
      rm -f -- "$pending_cache" "$degraded_cache"
      return 1
    }
    rm -f -- "$pending_cache"
    verdict="$("$PYTHON_BIN" "$NETWORK_QUALITY_HELPER" \
      --pid "$pid" --deep --cache-file "$pending_cache" --write-cache --contract-only \
      2>/dev/null || printf 'unknown\n')"
    if [ "$verdict" = "ready" ]; then
      probe_token="$(network_cache_probe_token "$pending_cache" 2>/dev/null || true)"
      if [ -z "$probe_token" ]; then
        verdict="cache_identity_invalid"
        success_streak=0
        success_token=""
      elif [ "$probe_token" = "$success_token" ]; then
        success_streak=$((success_streak + 1))
      else
        success_streak=1
        success_token="$probe_token"
      fi
      if [ "$success_streak" -ge 2 ]; then
        rm -f -- "$degraded_cache"
        mv -f -- "$pending_cache" "$NETWORK_CACHE_FILE" || return 1
        DEEP_NETWORK_VERDICT="ready"
        echo "Claude Science sandbox egress canary passed twice consecutively (anonymous, non-billable HTTPS HEAD requests to PyPI)."
        return 0
      fi
    fi
    if [ "$verdict" != "ready" ]; then
      success_streak=0
      success_token=""
      if [ "$verdict" != "cache_identity_invalid" ] && [ -f "$pending_cache" ]; then
        mv -f -- "$pending_cache" "$degraded_cache" || true
      fi
    fi
    [ "$attempt" -lt 4 ] || break
    case "$verdict" in
      egress_daemon_mount_io_busy|egress_daemon_busy)
        delay=$((attempt * 2))
        echo "Claude Science daemon is temporarily busy; retrying the sandbox protocol check in ${delay}s (${attempt}/4)." >&2
        ;;
      *)
        delay=2
        ;;
    esac
    sleep "$delay"
  done
  rm -f -- "$pending_cache"
  if [ -f "$degraded_cache" ]; then
    mv -f -- "$degraded_cache" "$NETWORK_CACHE_FILE" || true
  fi
  if [ "$verdict" = "ready" ]; then
    verdict="insufficient_consecutive_successes"
  fi
  DEEP_NETWORK_VERDICT="$verdict"
  case "$verdict" in
    egress_daemon_mount_io_busy)
      echo "Warning: Claude Science ports and sandbox forwarders exist, but its event loop is blocked on WSL-mounted filesystem I/O. The daemon was kept running; wait for MCP warmup or move high-I/O workspaces to WSL ext4, then refresh status. No model request was made." >&2
      ;;
    egress_daemon_busy)
      echo "Warning: Claude Science ports and sandbox forwarders exist, but its event loop remained busy during protocol handshakes. The daemon was kept running for a later retry; no model request was made." >&2
      ;;
    *)
      if [ -f "$NETWORK_CACHE_FILE" ]; then
        echo "Warning: Claude Science local services started, but sandbox egress quality is $verdict after bounded retries. The last safe degraded result was cached for diagnostics; no model request was made." >&2
      else
        echo "Warning: Claude Science local services started, but sandbox egress quality is $verdict after bounded retries. No green cache was published; no model request was made." >&2
      fi
      ;;
  esac
  return 1
}

sanitize_daemon_proxy_environment() {
  local state
  state="$(current_proxy_state)"
  case "$state" in
    direct)
      echo "Claude Science outbound proxy: direct environment"
      ;;
    reachable)
      echo "Claude Science outbound proxy: configured endpoint is reachable"
      ;;
    unreachable|invalid|conflict)
      # This changes only the child daemon environment. It does not modify the
      # WSL/Windows proxy, VPN, DNS, certificates, hosts, or any user file.
      unset HTTP_PROXY http_proxy HTTPS_PROXY https_proxy ALL_PROXY all_proxy
      echo "Warning: removed unusable/conflicting proxy variables from the new Claude Science daemon environment; system proxy settings were not changed." >&2
      ;;
    *)
      echo "CSA could not validate the Claude Science outbound proxy environment; refusing a blind start." >&2
      return 1
      ;;
  esac
}

check_bridge_health() {
  local payload
  payload="$(curl --noproxy '*' -fsS --connect-timeout 0.4 --max-time 1 "http://127.0.0.1:$PROXY_PORT/health" 2>/dev/null)" || return 1
  "$PYTHON_BIN" -c '
import json, os, sys
try:
    health = json.loads(sys.argv[5])
except Exception:
    raise SystemExit(1)
expected = os.path.realpath(sys.argv[1])
actual = os.path.realpath(str(health.get("source_path") or ""))
identity = health.get("runtime_identity") or {}
valid = (
    health.get("status") == "ok"
    and actual == expected
    and identity.get("schemaVersion") == 1
    and identity.get("component") == "bridge"
    and identity.get("managed") is True
    and identity.get("runtimeId") == sys.argv[2]
    and identity.get("version") == sys.argv[3]
    and identity.get("buildId") == sys.argv[4][:16].lower()
    and str(identity.get("sourceSha256") or "").lower() == sys.argv[4].lower()
    and isinstance(identity.get("pid"), int)
    and identity.get("pid") > 0
    and "health" in (identity.get("capabilities") or [])
    and os.path.realpath(str(identity.get("sourcePath") or "")) == expected
)
raise SystemExit(0 if valid else 1)
' "$BRIDGE_PROXY" "$CSA_BRIDGE_RUNTIME_ID" "$CSA_BRIDGE_VERSION" "$CSA_BRIDGE_SOURCE_SHA256" "$payload" >/dev/null 2>&1
}

wait_bridge_health() {
  local timeout="${1:-12}"
  local deadline=$((SECONDS + timeout))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if check_bridge_health; then
      # Require a second successful check so a process that only binds briefly
      # is not reported as a successful Bridge start.
      sleep 0.75
      if check_bridge_health; then
        return 0
      fi
    fi
    sleep 0.35
  done
  return 1
}

bridge_listener_pids() {
  ss -ltnp "sport = :$PROXY_PORT" 2>/dev/null \
    | grep -o 'pid=[0-9]*' \
    | cut -d= -f2 \
    | sort -u
}

claude_listener_pids() {
  for port in "$CLAUDE_SCIENCE_PORT" 8766; do
    listener_pids_for_port "$port"
  done | sort -u
}

managed_claude_pid() {
  local pid="$1" executable raw_executable
  local -a argv=()
  [ -r "/proc/$pid/cmdline" ] || return 1
  raw_executable="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
  executable="${raw_executable% (deleted)}"
  case "$executable" in
    "$CSA_CLAUDE_ROOT"/patched/*/claude-science|"$LEGACY_STATE_DIR"/patched/claude-science) ;;
    *) return 1;;
  esac
  mapfile -d '' -t argv <"/proc/$pid/cmdline" 2>/dev/null || true
  [ "${argv[0]:-}" = "$executable" ] && [ "${argv[1]:-}" = "serve" ]
}

process_start_ticks_shell() {
  local pid="$1" payload suffix
  payload="$(<"/proc/$pid/stat")" 2>/dev/null || return 1
  suffix="${payload##*) }"
  set -- $suffix
  case "${20:-}" in ''|*[!0-9]*) return 1;; esac
  printf '%s\n' "${20}"
}

managed_claude_token() {
  local pid="$1" before after
  before="$(process_start_ticks_shell "$pid")" || return 1
  managed_claude_pid "$pid" || return 1
  after="$(process_start_ticks_shell "$pid")" || return 1
  [ "$before" = "$after" ] || return 1
  printf '%s:%s\n' "$pid" "$before"
}

process_threads_signalable() {
  local pid="$1" task_dir payload suffix state count=0
  for task_dir in "/proc/$pid"/task/[0-9]*; do
    [ -d "$task_dir" ] || continue
    payload="$(<"$task_dir/stat")" 2>/dev/null || return 1
    suffix="${payload##*) }"
    state="${suffix%% *}"
    case "$state" in
      R|S|I) ;;
      *) return 1;;
    esac
    count=$((count + 1))
  done
  [ "$count" -gt 0 ]
}

managed_claude_signal_token() {
  local pid="$1" before after
  before="$(managed_claude_token "$pid")" || return 1
  process_threads_signalable "$pid" || return 1
  after="$(managed_claude_token "$pid")" || return 1
  [ "$before" = "$after" ] || return 1
  printf '%s\n' "$before"
}

exact_executable_serve_tokens() {
  local expected_bin="$1" expected_executable process_dir pid executable stat suffix start_ticks
  local -a argv=()
  expected_executable="$(readlink -f "$expected_bin" 2>/dev/null || true)"
  [ -n "$expected_executable" ] || return 0
  for process_dir in /proc/[0-9]*; do
    [ -d "$process_dir" ] || continue
    pid="${process_dir##*/}"
    executable="$(readlink -f "$process_dir/exe" 2>/dev/null || true)"
    [ "$executable" = "$expected_executable" ] || continue
    argv=()
    mapfile -d '' -t argv <"$process_dir/cmdline" 2>/dev/null || true
    [ "${argv[1]:-}" = "serve" ] || continue
    stat="$(<"$process_dir/stat")" 2>/dev/null || continue
    suffix="${stat##*) }"
    # starttime is field 22; suffix begins at field 3 (process state).
    set -- $suffix
    start_ticks="${20:-}"
    case "$start_ticks" in ''|*[!0-9]*) continue;; esac
    printf '%s:%s\n' "$pid" "$start_ticks"
  done
}

candidate_token_is_baseline() {
  local token="$1"
  printf '%s\n' "${CLAUDE_CANDIDATE_BASELINE_TOKENS:-}" \
    | grep -Fx -- "$token" >/dev/null 2>&1
}

exact_executable_signal_token() {
  local expected_bin="$1" expected_token="$2" pid
  pid="${expected_token%%:*}"
  exact_executable_serve_tokens "$expected_bin" \
    | grep -Fx -- "$expected_token" >/dev/null 2>&1 || return 1
  process_threads_signalable "$pid" || return 1
  exact_executable_serve_tokens "$expected_bin" \
    | grep -Fx -- "$expected_token" >/dev/null 2>&1 || return 1
  printf '%s\n' "$expected_token"
}

cleanup_failed_candidate_processes() {
  [ "${CLAUDE_CANDIDATE_LAUNCHED:-0}" = "1" ] || return 0
  [ -n "${PATCHED_BIN:-}" ] || return 0
  local token pid current_tokens deadline candidate_process_found
  current_tokens="$(exact_executable_serve_tokens "$PATCHED_BIN" || true)"
  for token in $current_tokens; do
    candidate_token_is_baseline "$token" && continue
    pid="${token%%:*}"
    # Recheck the PID+start-time token immediately before signalling so PID
    # reuse cannot redirect cleanup to an unrelated process.
    if exact_executable_signal_token "$PATCHED_BIN" "$token" >/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  deadline=$((SECONDS + 3))
  while [ "$SECONDS" -lt "$deadline" ]; do
    current_tokens="$(exact_executable_serve_tokens "$PATCHED_BIN" || true)"
    candidate_process_found=0
    for token in $current_tokens; do
      candidate_token_is_baseline "$token" && continue
      candidate_process_found=1
      pid="${token%%:*}"
      if exact_executable_signal_token "$PATCHED_BIN" "$token" >/dev/null; then
        kill "$pid" 2>/dev/null || true
      fi
    done
    [ "$candidate_process_found" = "0" ] && return 0
    sleep 0.1
  done
  current_tokens="$(exact_executable_serve_tokens "$PATCHED_BIN" || true)"
  for token in $current_tokens; do
    candidate_token_is_baseline "$token" && continue
    pid="${token%%:*}"
    if exact_executable_signal_token "$PATCHED_BIN" "$token" >/dev/null; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
  deadline=$((SECONDS + 1))
  while [ "$SECONDS" -lt "$deadline" ]; do
    candidate_process_found=0
    for token in $(exact_executable_serve_tokens "$PATCHED_BIN" || true); do
      candidate_token_is_baseline "$token" || candidate_process_found=1
    done
    [ "$candidate_process_found" = "0" ] && return 0
    sleep 0.05
  done
  return 1
}

running_claude_binary() {
  local pid executable
  for pid in $(claude_listener_pids || true); do
    case "$pid" in ''|*[!0-9]*) continue;; esac
    executable="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)"
    case "$executable" in
      "$CSA_CLAUDE_ROOT"/patched/*/claude-science|"$LEGACY_STATE_DIR"/patched/claude-science)
        if [ -x "$executable" ]; then
          printf '%s\n' "$executable"
          return 0
        fi
        ;;
    esac
  done
}

check_claude_health() {
  local expected_bin="$1" pid cmd executable expected_executable
  expected_executable="$(readlink -f "$expected_bin" 2>/dev/null || true)"
  pid="$(claude_primary_pid)" || return 1
  [ -r "/proc/$pid/cmdline" ] || return 1
  executable="$(readlink -f "/proc/$pid/exe" 2>/dev/null || true)"
  [ -n "$expected_executable" ] && [ "$executable" = "$expected_executable" ] || return 1
  cmd="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
  [[ "$cmd" == *"$expected_bin serve"* ]] || return 1
  curl --noproxy '*' -sS -o /dev/null --connect-timeout 0.5 --max-time 1 \
    "http://127.0.0.1:$CLAUDE_SCIENCE_PORT/" 2>/dev/null
}

wait_claude_health() {
  local expected_bin="$1"
  local timeout="${2:-15}"
  local deadline=$((SECONDS + timeout))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if check_claude_health "$expected_bin"; then
      sleep 0.75
      if check_claude_health "$expected_bin"; then
        return 0
      fi
    fi
    sleep 0.35
  done
  return 1
}

launch_claude_daemon() {
  local executable="$1" base_url="$2" runtime_working_dir
  runtime_working_dir="$(dirname "$executable")"
  [ -x "$executable" ] || return 1
  [ -d "$runtime_working_dir" ] || return 1
  # The portable package normally lives on /mnt/c or /mnt/e (DrvFS/9P).
  # Claude Science inherits its launch cwd and performs substantial workspace
  # discovery during MCP warmup.  Starting from the managed ext4 runtime keeps
  # that control-plane I/O off the Windows mount without moving user projects.
  (
    cd -P "$runtime_working_dir"
    ANTHROPIC_BASE_URL="$base_url" "$executable" serve \
      --port "$CLAUDE_SCIENCE_PORT" --no-browser --detached --no-auto-update
  )
}

stop_existing_claude_for_activation() {
  local claude_pids claude_tokens="" pid token cmd deadline
  PREVIOUS_RUNNING_CLAUDE_BIN="$(running_claude_binary || true)"
  claude_pids="$(claude_listener_pids || true)"
  for pid in $claude_pids; do
    case "$pid" in ''|*[!0-9]*) continue;; esac
    token="$(managed_claude_signal_token "$pid" || true)"
    if [ -z "$token" ]; then
      echo "Claude Science port owner PID $pid is unverified or in an unsafe scheduler state; CSA will not stop it or mutate Bridge." >&2
      return 1
    fi
    claude_tokens="${claude_tokens}${token}"$'\n'
  done
  [ -n "$claude_pids" ] || return 0

  # Close the TOCTOU window immediately before TERM.  If the daemon entered
  # D/T/Z/unknown state after preflight, abort before Bridge or runtime pointers
  # are touched.
  for token in $claude_tokens; do
    pid="${token%%:*}"
    if [ "$(managed_claude_signal_token "$pid" || true)" != "$token" ]; then
      echo "Claude Science identity/state changed before TERM; CSA left Bridge and runtime pointers unchanged." >&2
      return 1
    fi
  done
  CLAUDE_STOPPED_FOR_ACTIVATION=1
  for token in $claude_tokens; do
    pid="${token%%:*}"
    if [ "$(managed_claude_signal_token "$pid" || true)" != "$token" ]; then
      echo "Claude Science identity/state changed at TERM; CSA left Bridge and runtime pointers unchanged." >&2
      return 1
    fi
    cmd="$(tr '\0' ' ' 2>/dev/null <"/proc/$pid/cmdline" || true)"
    if [[ "$cmd" == *"claude-science"*"serve"* ]]; then
      kill "$pid" 2>/dev/null || true
    fi
  done

  deadline=$((SECONDS + 5))
  while [ "$SECONDS" -lt "$deadline" ] && [ -n "$(claude_listener_pids || true)" ]; do
    sleep 0.25
  done
  if [ -n "$(claude_listener_pids || true)" ]; then
    for token in $claude_tokens; do
      pid="${token%%:*}"
      if [ "$(managed_claude_signal_token "$pid" || true)" = "$token" ]; then
        cmd="$(tr '\0' ' ' 2>/dev/null <"/proc/$pid/cmdline" || true)"
        if [[ "$cmd" == *"claude-science"*"serve"* ]]; then
          kill -9 "$pid" 2>/dev/null || true
        fi
      fi
    done
  fi
  if [ -n "$(claude_listener_pids || true)" ]; then
    echo "Existing Claude Science listeners did not stop safely within 5 seconds; CSA left Bridge unchanged." >&2
    return 1
  fi
}

verified_bridge_listener() {
  local pid="$1" payload
  payload="$(curl --noproxy '*' -fsS --connect-timeout 0.4 --max-time 1 \
    "http://127.0.0.1:$PROXY_PORT/health" 2>/dev/null)" || return 1
  "$PYTHON_BIN" - "$pid" "$CSA_STATE_ROOT" "$LEGACY_STATE_DIR" "$payload" <<'PY' >/dev/null 2>&1
import hashlib
import json
import os
import sys

pid = int(sys.argv[1])
state_root = os.path.realpath(sys.argv[2])
legacy_root = os.path.realpath(sys.argv[3])
health = json.loads(sys.argv[4])
identity = health.get("runtime_identity") or {}
source = os.path.realpath(str(identity.get("sourcePath") or health.get("source_path") or ""))
versions_root = os.path.join(state_root, "runtime", "bridge", "versions") + os.sep
managed_valid = (
    health.get("status") == "ok"
    and identity.get("schemaVersion") == 1
    and identity.get("component") == "bridge"
    and identity.get("managed") is True
    and identity.get("pid") == pid
    and source.startswith(versions_root)
    and os.path.isfile(source)
)
if managed_valid:
    with open(source, "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()
    managed_valid = digest.casefold() == str(identity.get("sourceSha256") or "").casefold()

legacy_valid = False
if not identity and health.get("status") == "ok" and os.path.isfile(source):
    try:
        command = [item.decode(errors="replace") for item in open(f"/proc/{pid}/cmdline", "rb").read().split(b"\0") if item]
        package_root = os.path.dirname(source)
        # Older v0.1.x portable manifests were written by Windows PowerShell
        # and may contain a UTF-8 BOM.  Accept that historical encoding while
        # keeping every other legacy-owner check strict.
        manifest = json.load(open(os.path.join(package_root, "manifest.json"), encoding="utf-8-sig"))
        legacy_valid = (
            bool(command)
            and os.path.normpath(command[0]).startswith(os.path.join(legacy_root, "venv") + os.sep)
            and any(os.path.realpath(item) == source for item in command if item.endswith("proxy.py"))
            and manifest.get("schemaVersion") == 1
            and manifest.get("product") == "CSA - Claude Science Assistant"
            and manifest.get("profile") in {"release", "debug"}
            and "proxy.py" in (manifest.get("expectedRootFiles") or [])
            and os.path.isfile(os.path.join(package_root, "requirements.txt"))
            and os.path.isfile(os.path.join(package_root, "setup-token.py"))
        )
    except (OSError, ValueError, TypeError):
        legacy_valid = False
raise SystemExit(0 if managed_valid or legacy_valid else 1)
PY
}

stop_stale_bridge_listener() {
  local pids pid stopped=0
  pids="$(bridge_listener_pids || true)"
  if [ -z "$pids" ]; then
    if ss -ltn "sport = :$PROXY_PORT" 2>/dev/null | grep -q LISTEN; then
      echo "Port $PROXY_PORT is occupied, but CSA cannot identify its owner. Stop that process before starting Bridge." >&2
      return 1
    fi
    return 0
  fi
  for pid in $pids; do
    if ! verified_bridge_listener "$pid"; then
      echo "Port $PROXY_PORT is occupied by an unverified process (PID $pid); CSA will not stop it." >&2
      return 1
    fi
  done
  for pid in $pids; do
    if ! verified_bridge_listener "$pid"; then
      echo "Bridge owner changed during verification; refusing to signal PID $pid." >&2
      return 1
    fi
    echo "Stopping verified stale CSA Bridge listener (PID $pid)"
    kill "$pid" 2>/dev/null || true
    stopped=1
  done
  if [ "$stopped" = "1" ]; then
    local deadline=$((SECONDS + 3))
    while [ "$SECONDS" -lt "$deadline" ]; do
      if ! ss -ltn "sport = :$PROXY_PORT" 2>/dev/null | grep -q LISTEN; then
        return 0
      fi
      sleep 0.2
    done
    echo "Stale CSA Bridge did not release port $PROXY_PORT." >&2
    return 1
  fi
}

rotate_bridge_log() {
  if [ "$LOG_FILE" = "/dev/null" ] || [ ! -f "$LOG_FILE" ]; then
    return 0
  fi
  local size
  size="$(stat -c %s "$LOG_FILE" 2>/dev/null || printf 0)"
  if [ "${size:-0}" -le $((50 * 1024 * 1024)) ]; then
    return 0
  fi
  rm -f "$LOG_FILE.1" 2>/dev/null || true
  mv -f "$LOG_FILE" "$LOG_FILE.1"
  : >"$LOG_FILE"
  echo "Rotated Bridge log at 50 MB (kept one backup: $LOG_FILE.1)"
}

systemd_runtime_available() {
  [ "${CSA_DISABLE_SYSTEMD:-0}" != "1" ] \
    && [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" = "systemd" ]
}

service_matches_runtime() {
  if ! systemd_runtime_available; then
    return 0
  fi
  local unit
  unit="$(systemctl --user cat claude-science-bridge.service 2>/dev/null || true)"
  grep -F -- "$BRIDGE_PROXY" <<<"$unit" >/dev/null 2>&1 \
    && grep -F -- "CSA_BRIDGE_RUNTIME_ID=$CSA_BRIDGE_RUNTIME_ID" <<<"$unit" >/dev/null 2>&1 \
    && grep -F -- "CSA_BRIDGE_SOURCE_SHA256=$CSA_BRIDGE_SOURCE_SHA256" <<<"$unit" >/dev/null 2>&1 \
    && grep -F -- "PROXY_PORT=$PROXY_PORT" <<<"$unit" >/dev/null 2>&1 \
    && grep -F -- "UnsetEnvironment=HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy" <<<"$unit" >/dev/null 2>&1
}

start_fallback_proxy() {
  if check_bridge_health; then
    echo "WSL BYOK proxy already healthy on 127.0.0.1:$PROXY_PORT"
    return 0
  fi
  echo "Starting fallback WSL BYOK proxy on 127.0.0.1:$PROXY_PORT"
  stop_stale_bridge_listener
  rotate_bridge_log
  local proxy_pid
  if ! proxy_pid="$("$PYTHON_BIN" - "$PYTHON_BIN" "$BRIDGE_PROXY" "$CSA_BRIDGE_ROOT/current" "$LOG_FILE" <<'PY'
import os
import subprocess
import sys

python_bin, proxy_script, project_dir, log_file = sys.argv[1:]
with open(log_file, "ab", buffering=0) as log:
    process = subprocess.Popen(
        [python_bin, proxy_script],
        cwd=project_dir,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        close_fds=True,
        env=os.environ.copy(),
    )
print(process.pid)
PY
  )"; then
    echo "Fallback WSL proxy process could not be launched." >&2
    return 1
  fi
  echo "Fallback WSL BYOK proxy process started (PID $proxy_pid)"
  if ! wait_bridge_health 12; then
    echo "Fallback WSL proxy did not start on 127.0.0.1:$PROXY_PORT. Last log lines:" >&2
    if [ "$LOG_FILE" != "/dev/null" ]; then
      tail -80 "$LOG_FILE" >&2 || true
    else
      echo "Proxy log is unavailable because no writable WSL log path was found." >&2
    fi
    if [ -r "/proc/$proxy_pid/cmdline" ]; then
      local launched_cmdline
      launched_cmdline="$(tr '\0' ' ' <"/proc/$proxy_pid/cmdline" 2>/dev/null || true)"
      if [[ "$launched_cmdline" == *"$BRIDGE_PROXY"* ]]; then
        kill "$proxy_pid" 2>/dev/null || true
      fi
    fi
    return 1
  fi
}

start_systemd_proxy() {
  local unit_ready=0
  if service_matches_runtime; then
    unit_ready=1
  elif PROXY_PORT="$PROXY_PORT" PYTHON="$PYTHON_BIN" CSA_STATE_ROOT="$CSA_STATE_ROOT" \
    CSA_PACKAGE_DIR="$PROJECT_DIR" CSA_BRIDGE_RUNTIME_ID="$CSA_BRIDGE_RUNTIME_ID" \
    CSA_BRIDGE_VERSION="$CSA_BRIDGE_VERSION" CSA_BRIDGE_SOURCE_SHA256="$CSA_BRIDGE_SOURCE_SHA256" \
    bash "$SCRIPT_DIR/install-wsl-bridge-service.sh" >/dev/null; then
    unit_ready=1
  else
    echo "Warning: failed to install/update systemd user service; falling back to direct WSL proxy start." >&2
  fi

  if [ "$unit_ready" != "1" ]; then
    return 1
  fi

  # Remove only exact legacy fallback Bridge processes before handing ownership to systemd.
  systemctl --user stop claude-science-bridge.service >/dev/null 2>&1 || true
  stop_stale_bridge_listener
  rotate_bridge_log
  if ! systemctl --user restart claude-science-bridge.service; then
    echo "Warning: systemd user service restart failed; falling back to direct WSL proxy start." >&2
    return 1
  fi
  echo "WSL BYOK proxy managed by systemd user service on 127.0.0.1:$PROXY_PORT"
  if ! wait_bridge_health 8; then
    systemctl --user stop claude-science-bridge.service >/dev/null 2>&1 || true
    echo "WSL proxy service did not start on 127.0.0.1:$PROXY_PORT. Last log lines:" >&2
    tail -80 "$LOG_DIR/wsl-proxy.log" >&2 || true
    return 1
  fi
}

# Unit tests source the function layer without entering the lifecycle mutation
# path.  Normal execution always continues below.
if [ "${BASH_SOURCE[0]}" != "$0" ]; then
  return 0
fi

if [ "${#PROXY_PORT}" -ne 4 ]; then
  echo "PROXY_PORT must be four digits for byte-length-preserving URL patches. Current: $PROXY_PORT" >&2
  exit 1
fi

write_probe() {
  local dir="$1"
  local probe="$dir/.csa-write-test-$$"
  (: >"$probe") 2>/dev/null || return 1
  rm -f "$probe" 2>/dev/null || true
}

if ! write_probe "${TMPDIR:-/tmp}"; then
  echo "WSL temporary directory is not writable: ${TMPDIR:-/tmp}. Run 'wsl --shutdown', reopen Ubuntu, and retry. If it remains read-only, repair or recreate this WSL distro." >&2
  exit 1
fi
if ! write_probe "$HOME"; then
  echo "WSL user home is not writable: $HOME. CSA cannot create its runtime, logs, or Bridge configuration until the WSL filesystem is repaired." >&2
  exit 1
fi

if mkdir -p "$LOG_DIR" 2>/dev/null && : >>"$LOG_FILE" 2>/dev/null; then
  :
else
  echo "Warning: WSL log directory is not writable; proxy fallback logs will be discarded." >&2
  LOG_FILE="/dev/null"
fi

if ! command -v flock >/dev/null 2>&1; then
  echo "CSA lifecycle lock requires flock (util-linux); refusing an unlocked runtime mutation." >&2
  exit 1
fi
LIFECYCLE_LOCK_FILE="$CSA_STATE_ROOT/runtime/lifecycle.lock"
mkdir -p "$(dirname "$LIFECYCLE_LOCK_FILE")"
exec {CSA_LIFECYCLE_LOCK_FD}>"$LIFECYCLE_LOCK_FILE"
if ! flock -w "${CSA_LIFECYCLE_LOCK_TIMEOUT:-8}" "$CSA_LIFECYCLE_LOCK_FD"; then
  echo "Another CSA lifecycle operation owns $LIFECYCLE_LOCK_FILE; wait for it to finish and retry." >&2
  exit 1
fi

BRIDGE_POINTER_CHANGED=0
BRIDGE_PREVIOUS_RUNTIME=""
BRIDGE_CANDIDATE_RUNTIME=""
BRIDGE_VALIDATED=0
CLAUDE_POINTER_CHANGED=0
CLAUDE_PREVIOUS_RUNTIME=""
CLAUDE_CANDIDATE_RUNTIME=""
CLAUDE_VALIDATED=0
CLAUDE_STOPPED_FOR_ACTIVATION=0
PREVIOUS_RUNNING_CLAUDE_BIN=""
CLAUDE_CANDIDATE_LAUNCHED=0
CLAUDE_CANDIDATE_BASELINE_TOKENS=""
START_COMPLETED=0

rollback_runtime_pointer() {
  local root="$1" previous="$2" candidate="$3" label="$4" current
  current="$(csa_current_target "$root")"
  if [ -z "$candidate" ] || [ "$current" != "$candidate" ]; then
    echo "Refusing to roll back $label: current pointer no longer belongs to this activation transaction." >&2
    return 1
  fi
  if [ -n "$previous" ]; then
    if [ -d "$previous" ]; then
      csa_atomic_symlink "$previous" "$root/current"
      if [ -d "$candidate" ] && [ "$candidate" != "$previous" ]; then
        csa_atomic_symlink "$candidate" "$root/previous"
      fi
      echo "Restored previous $label runtime pointer after failed activation." >&2
      return 0
    fi
    echo "Failed to restore the previous $label runtime pointer." >&2
    return 1
  fi

  if [ -L "$root/current" ]; then
    rm -f "$root/current"
    echo "Removed the failed first $label runtime pointer; no previous managed runtime existed." >&2
    return 0
  fi
  echo "Could not safely clear the failed first $label runtime pointer." >&2
  return 1
}

restore_runtime_after_failure() {
  local exit_code=$?
  trap - EXIT
  if [ "$exit_code" -ne 0 ] && [ "$START_COMPLETED" != "1" ]; then
    set +e
    set +u
    cleanup_failed_candidate_processes || \
      echo "Failed Claude Science candidate did not exit within 3 seconds." >&2
    if [ "$CLAUDE_POINTER_CHANGED" = "1" ]; then
      rollback_runtime_pointer \
        "$CSA_CLAUDE_ROOT" "$CLAUDE_PREVIOUS_RUNTIME" "$CLAUDE_CANDIDATE_RUNTIME" \
        "Claude Science" || true
    fi
    if [ "$BRIDGE_VALIDATED" != "1" ] \
      && [ "$BRIDGE_POINTER_CHANGED" = "1" ]; then
      if rollback_runtime_pointer \
        "$CSA_BRIDGE_ROOT" "$BRIDGE_PREVIOUS_RUNTIME" "$BRIDGE_CANDIDATE_RUNTIME" \
        "Bridge"; then
        if [ -n "$BRIDGE_PREVIOUS_RUNTIME" ] && csa_load_bridge_runtime_identity; then
          if systemd_runtime_available; then
            start_systemd_proxy >/dev/null 2>&1 || start_fallback_proxy >/dev/null 2>&1 || true
          else
            start_fallback_proxy >/dev/null 2>&1 || true
          fi
        fi
      fi
    fi
    # Restore the Bridge transaction before relaunching the previous Claude
    # daemon.  Otherwise a failed Bridge candidate can make rollback startup
    # fail even though the previous runtime is intact.
    if [ "$CLAUDE_STOPPED_FOR_ACTIVATION" = "1" ] \
      && [ -n "$PREVIOUS_RUNNING_CLAUDE_BIN" ] \
      && [ -x "$PREVIOUS_RUNNING_CLAUDE_BIN" ] \
      && [ -z "$(claude_listener_pids || true)" ]; then
      rollback_proxy_url="http://127.0.0.1:$PROXY_PORT"
      if launch_claude_daemon "$PREVIOUS_RUNNING_CLAUDE_BIN" "$rollback_proxy_url" \
        >/dev/null 2>&1 \
        && wait_claude_health "$PREVIOUS_RUNNING_CLAUDE_BIN" 12; then
        echo "Restarted the previous Claude Science daemon after failed activation." >&2
      else
        echo "Failed to restart the previous Claude Science daemon after activation failure." >&2
      fi
    fi
  fi
  exit "$exit_code"
}
trap restore_runtime_after_failure EXIT

# Refuse a partial ForceRestart before Bridge is touched.  A Claude process in
# D/T/Z/unknown state cannot be stopped predictably; all thread states plus the
# PID start token are re-read from /proc around the ownership check.
preflight_claude_pids="$(claude_listener_pids || true)"
preflight_unsafe_pid=""
for pid in $preflight_claude_pids; do
  case "$pid" in ''|*[!0-9]*) continue;; esac
  if ! managed_claude_token "$pid" >/dev/null; then
    echo "Claude Science port is owned by an unverified process (PID $pid); CSA will not mutate Bridge or signal it." >&2
    exit 1
  fi
  if ! managed_claude_signal_token "$pid" >/dev/null; then
    preflight_unsafe_pid="$pid"
  fi
done
if [ -n "$preflight_unsafe_pid" ]; then
  if [ "${CSA_FORCE_RESTART:-0}" = "1" ] \
    || ! csa_load_bridge_runtime_identity >/dev/null 2>&1 \
    || ! check_bridge_health \
    || ! check_claude_network_contract; then
    echo "Claude Science PID $preflight_unsafe_pid is currently in an unsafe scheduler state for repair (for example D/p9 mount I/O). CSA left Claude Science, Bridge, WSL, runtime pointers, and unrelated ports unchanged; refresh and retry after I/O returns." >&2
    exit 1
  fi
  echo "Claude Science PID $preflight_unsafe_pid is temporarily unsafe to signal. Existing healthy local services were preserved without staging or mutating Bridge/runtime pointers."
  if record_deep_network_quality; then
    echo "Claude Science recovered and passed the network probe, but package migration was intentionally deferred; refresh once more to apply it safely."
  else
    echo "Claude Science remains locally available but external API readiness is degraded ($DEEP_NETWORK_VERDICT). Refresh after I/O returns; CSA did not restart WSL or touch unrelated port 2222."
  fi
  csa_print_bridge_identity
  START_COMPLETED=1
  exit 0
fi

# A full activation stops the verified Claude daemon before staging or
# restarting Bridge.  This makes the state transition one-way: if the daemon
# becomes uninterruptible, the operation fails while Bridge and both runtime
# pointers are still untouched.  Explicit Bridge-only operations retain their
# documented behavior and never attempt to stop Claude.
if [ "${CSA_BRIDGE_ONLY:-0}" != "1" ]; then
  stop_existing_claude_for_activation || exit 1
fi

csa_stage_bridge_runtime "$PROJECT_DIR" "$CSA_PACKAGE_VERSION"
BRIDGE_POINTER_CHANGED="$CSA_POINTER_CHANGED"
BRIDGE_PREVIOUS_RUNTIME="$CSA_PREVIOUS_RUNTIME"
BRIDGE_CANDIDATE_RUNTIME="$CSA_BRIDGE_RUNTIME_DIR"

start_current_bridge() {
  if check_bridge_health && [ "${CSA_FORCE_RESTART:-0}" != "1" ]; then
    echo "WSL BYOK proxy already healthy on 127.0.0.1:$PROXY_PORT"
    return 0
  fi
  if systemd_runtime_available; then
    start_systemd_proxy || start_fallback_proxy
  else
    if [ "${CSA_FORCE_RESTART:-0}" = "1" ]; then
      stop_stale_bridge_listener
    fi
    start_fallback_proxy
  fi
}

if ! start_current_bridge; then
  echo "Managed Bridge candidate failed to start; activation will be rolled back." >&2
  exit 1
fi
BRIDGE_VALIDATED=1

if [ "${CSA_BRIDGE_ONLY:-0}" = "1" ]; then
  echo "Bridge-only restart complete; Claude Science was left running."
  csa_print_bridge_identity
  START_COMPLETED=1
  exit 0
fi

csa_stage_claude_runtime "$PROJECT_DIR"
CLAUDE_POINTER_CHANGED="$CSA_POINTER_CHANGED"
CLAUDE_PREVIOUS_RUNTIME="$CSA_PREVIOUS_RUNTIME"
CLAUDE_CANDIDATE_RUNTIME="$CSA_CLAUDE_RUNTIME_DIR"
SOURCE_BIN="$CSA_CLAUDE_RUNTIME_DIR/claude-science"
SOURCE_SHA="$CSA_CLAUDE_SOURCE_SHA256"
PATCH_PROFILE="byok-no-eager-mcp-warmup-v1"
if [ -n "$PATCH_DIR_OVERRIDE" ]; then
  PATCH_DIR="$PATCH_DIR_OVERRIDE"
else
  PATCH_DIR="$CSA_CLAUDE_ROOT/patched/${SOURCE_SHA}-${PROXY_PORT}-${PATCH_PROFILE}"
fi
PATCHED_BIN="$PATCH_DIR/claude-science"

mkdir -p "$PATCH_DIR"

echo "Using Claude Science Linux binary: $SOURCE_BIN"
"$SOURCE_BIN" --version 2>/dev/null || true

if [ "${CSA_FORCE_RESTART:-0}" != "1" ] \
  && check_bridge_health \
  && { ! systemd_runtime_available || service_matches_runtime; } \
  && check_claude_health "$PATCHED_BIN" \
  && check_claude_network_contract; then
  echo "Claude Science and WSL BYOK proxy are already running; using fast start path."
  if record_deep_network_quality && [ -x "$PATCHED_BIN" ]; then
    echo "Claude Science is ready on 127.0.0.1:$CLAUDE_SCIENCE_PORT. Use the launcher to open it."
  else
    echo "Claude Science local listeners remain available on 127.0.0.1:$CLAUDE_SCIENCE_PORT, but external API readiness is degraded ($DEEP_NETWORK_VERDICT). The launcher will not report the service ready until a later deep check passes twice consecutively."
  fi
  csa_print_bridge_identity
  START_COMPLETED=1
  exit 0
fi

if [ -n "$(claude_listener_pids || true)" ] && ! check_claude_network_contract; then
  echo "Existing Claude Science daemon failed its outbound proxy contract; replacing it through the controlled start path." >&2
fi
sanitize_daemon_proxy_environment

TOKEN_FILE="$HOME/.claude-science/.oauth-tokens/byok-user-000000000000000000.enc"
if [ -f "$HOME/.claude-science/encryption.key" ]; then
  if [ -f "$TOKEN_FILE" ]; then
    echo "Local fake OAuth token already exists"
  else
    echo "Refreshing local fake OAuth token"
    "$PYTHON_BIN" "$CSA_BRIDGE_ROOT/current/setup-token.py" >/dev/null
  fi
else
  echo "Warning: ~/.claude-science/encryption.key does not exist; fake OAuth token was not generated." >&2
fi

ACTUAL_SOURCE_SHA="$(sha256sum "$SOURCE_BIN" | awk '{print tolower($1)}')"
if [ "$ACTUAL_SOURCE_SHA" != "$SOURCE_SHA" ]; then
  echo "Managed Claude Science source changed after activation; refusing to patch it." >&2
  exit 1
fi
PATCH_MARKER="$PATCH_DIR/.claude-science.source.sha256"
PATCH_CACHE_KEY="$SOURCE_SHA:$PROXY_PORT:$PATCH_PROFILE"

if [ -x "$PATCHED_BIN" ] \
  && [ -f "$PATCH_MARKER" ] \
  && [ "$(cat "$PATCH_MARKER" 2>/dev/null || true)" = "$PATCH_CACHE_KEY" ] \
  && "$PATCHED_BIN" --help >/dev/null 2>&1; then
  echo "Patched Claude Science daemon cache is current"
else
  cp -f "$SOURCE_BIN" "$PATCHED_BIN"
  chmod +x "$PATCHED_BIN"

  TARGET="$PATCHED_BIN" PROXY_PORT="$PROXY_PORT" "$PYTHON_BIN" - <<'PY'
import os
import shutil
import stat
from pathlib import Path

target = Path(os.environ["TARGET"]).expanduser()
port = os.environ["PROXY_PORT"]
backup = target.with_name(target.name + ".byok-auth-original")

pairs = [
    (
        [b"https://api.anthropic.com"],
        f"http://127.00.00.001:{port}".encode(),
    ),
    (
        [b"https://api.anthropic.com/api/oauth/profile"],
        f"http://127.00.00.001:{port}/api/oauth/profile".encode(),
    ),
    (
        [b"https://api.anthropic.com/api/oauth/account"],
        f"http://127.00.00.001:{port}/api/oauth/account".encode(),
    ),
    (
        [b"https://api.anthropic.com/api/oauth/usage"],
        f"http://127.00.00.001:{port}/api/oauth/usage".encode(),
    ),
    (
        [
            b"https://platform.claude.com/v1/oauth/token",
            b"https://127.00.00.001:9877/api/oauth/token",
        ],
        f"http://127.000.000.01:{port}/api/oauth/token".encode(),
    ),
    # Claude Science 0.1.25 unconditionally calls QT9(V) during boot to
    # prewarm every bundled MCP connector.  On WSL workspaces under /mnt/c or
    # /mnt/e that can block Bun's main event loop in p9_client_rpc for 80-100s,
    # leaving all six proxy ports listening but unable to accept.  The locked
    # build contains an existing no-op ZT9 and this full 22-byte call context
    # occurs exactly once.  Patch only the managed copy; lazy connector startup
    # remains available when a connector is actually used.
    (
        [b"QT9(V),oe_(V,WG).catch"],
        b"ZT9(V),oe_(V,WG).catch",
    ),
]

for olds, new in pairs:
    for old in olds:
        if len(old) != len(new):
            raise SystemExit(f"length mismatch: {old!r} ({len(old)}) -> {new!r} ({len(new)})")

data = target.read_bytes()
counts = [(olds, new, sum(data.count(old) for old in olds), data.count(new)) for olds, new in pairs]
warmup_old = b"QT9(V),oe_(V,WG).catch"
warmup_new = b"ZT9(V),oe_(V,WG).catch"
if data.count(b"function QT9(z)") != 1 or data.count(b"function ZT9(z){}") != 1:
    raise SystemExit("Unsupported Claude Science daemon build; eager/no-op warmup function identity changed")
if data.count(warmup_old) + data.count(warmup_new) != 1:
    raise SystemExit("Unsupported Claude Science daemon build; eager MCP warmup call identity is not unique")
missing = [
    " or ".join(old.decode(errors="replace") for old in olds)
    for olds, new, old_count, new_count in counts
    if old_count == 0 and new_count == 0
]
if missing:
    raise SystemExit(
        "Unsupported Claude Science daemon build; expected version-locked patch pattern(s) not found:\n"
        + "\n".join(f"  - {item}" for item in missing)
    )

if any(old_count > 0 for _, _, old_count, _ in counts) and not backup.exists():
    shutil.copy2(target, backup)

patched = 0
with target.open("r+b") as f:
    for olds, new, _, _ in counts:
        for old in olds:
            start = 0
            while True:
                idx = data.find(old, start)
                if idx < 0:
                    break
                f.seek(idx)
                f.write(new)
                patched += 1
                start = idx + len(old)

target.chmod(target.stat().st_mode | stat.S_IXUSR)
after = target.read_bytes()
for olds, new, _, _ in counts:
    for old in olds:
        if after.count(old) != 0:
            raise SystemExit(f"patch verification failed; original URL still present: {old.decode()}")
    if after.count(new) == 0:
        raise SystemExit(f"patch verification failed; replacement URL missing: {new.decode()}")
if after.count(warmup_old) != 0 or after.count(warmup_new) != 1:
    raise SystemExit("patch verification failed; eager MCP warmup call was not replaced exactly once")

print(f"Patched managed runtime byte occurrence(s): {patched}")
PY

  if ! "$PATCHED_BIN" --help >/dev/null 2>&1; then
    echo "Patched daemon failed executable check." >&2
    exit 1
  fi
  printf '%s\n' "$PATCH_CACHE_KEY" > "$PATCH_MARKER"
fi

PROXY_URL="http://127.0.0.1:$PROXY_PORT"
CLAUDE_CANDIDATE_BASELINE_TOKENS="$(exact_executable_serve_tokens "$PATCHED_BIN" || true)"
CLAUDE_CANDIDATE_LAUNCHED=1
launch_claude_daemon "$PATCHED_BIN" "$PROXY_URL"
if ! wait_claude_health "$PATCHED_BIN" 15; then
  echo "Claude Science candidate did not become healthy on 127.0.0.1:$CLAUDE_SCIENCE_PORT." >&2
  exit 1
fi
if ! wait_claude_network_contract 5; then
  echo "Claude Science candidate started locally but failed its outbound proxy contract." >&2
  exit 1
fi
csa_atomic_symlink "$PATCH_DIR" "$CSA_CLAUDE_ROOT/patched-current"
if record_deep_network_quality; then
  CLAUDE_VALIDATED=1
  echo "Started Claude Science validated patched copy:"
  echo "  daemon: $PATCHED_BIN"
  echo "  ANTHROPIC_BASE_URL=$PROXY_URL"
  echo "Claude Science is ready on 127.0.0.1:$CLAUDE_SCIENCE_PORT. Use the launcher to open it."
else
  echo "Started Claude Science patched copy with degraded network quality:"
  echo "  daemon: $PATCHED_BIN"
  echo "  ANTHROPIC_BASE_URL=$PROXY_URL"
  echo "Claude Science local listeners remain available on 127.0.0.1:$CLAUDE_SCIENCE_PORT, but external API readiness is degraded ($DEEP_NETWORK_VERDICT). The process was kept running without restarting WSL or touching unrelated port 2222."
fi
csa_print_bridge_identity
START_COMPLETED=1
