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
STATE_DIR="$HOME/.local/share/claude-science-api-bridge"
BUNDLED_CLAUDE_DIR="$PROJECT_DIR/vendor/claude-science/linux-x64"
BUNDLED_CLAUDE_BIN="$BUNDLED_CLAUDE_DIR/claude-science"
BUNDLED_CLAUDE_SHA="$BUNDLED_CLAUDE_DIR/claude-science.sha256"
MANAGED_CLAUDE_DIR="$STATE_DIR/bin"
MANAGED_CLAUDE_BIN="$MANAGED_CLAUDE_DIR/claude-science"
PATCH_DIR="${PATCH_DIR:-$HOME/.local/share/claude-science-api-bridge/patched}"
PATCHED_BIN="${PATCHED_BIN:-$PATCH_DIR/claude-science}"
LOG_DIR="$HOME/.claude-science/logs"
LOG_FILE="$LOG_DIR/wsl-proxy.log"

if [ -x "$HOME/.local/share/claude-science-api-bridge/venv/bin/python" ]; then
  PYTHON_BIN="${PYTHON:-$HOME/.local/share/claude-science-api-bridge/venv/bin/python}"
else
  PYTHON_BIN="${PYTHON:-python3}"
fi

check_bridge_health() {
  local payload
  payload="$(curl -fsS --connect-timeout 0.4 --max-time 1 "http://127.0.0.1:$PROXY_PORT/health" 2>/dev/null)" || return 1
  "$PYTHON_BIN" -c '
import json, os, sys
try:
    health = json.loads(sys.argv[2])
except Exception:
    raise SystemExit(1)
expected = os.path.realpath(sys.argv[1])
actual = os.path.realpath(str(health.get("source_path") or ""))
raise SystemExit(0 if health.get("status") == "ok" and actual == expected else 1)
' "$PROJECT_DIR/proxy.py" "$payload" >/dev/null 2>&1
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

stop_stale_bridge_listener() {
  local pids pid cmdline stopped=0
  pids="$(bridge_listener_pids || true)"
  if [ -z "$pids" ]; then
    if ss -ltn "sport = :$PROXY_PORT" 2>/dev/null | grep -q LISTEN; then
      echo "Port $PROXY_PORT is occupied, but CSA cannot identify its owner. Stop that process before starting Bridge." >&2
      return 1
    fi
    return 0
  fi
  for pid in $pids; do
    cmdline="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
    case "$cmdline" in
      *"/proxy.py"*)
        echo "Stopping stale CSA Bridge listener (PID $pid)"
        kill "$pid" 2>/dev/null || true
        stopped=1
        ;;
      *)
        echo "Port $PROXY_PORT is occupied by a non-CSA process (PID $pid)." >&2
        return 1
        ;;
    esac
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

service_matches_project() {
  if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" != "systemd" ]; then
    return 0
  fi
  systemctl --user cat claude-science-bridge.service 2>/dev/null | grep -F -- "$PROJECT_DIR/proxy.py" >/dev/null 2>&1
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
  if ! proxy_pid="$("$PYTHON_BIN" - "$PYTHON_BIN" "$PROJECT_DIR/proxy.py" "$PROJECT_DIR" "$LOG_FILE" <<'PY'
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
      if [[ "$launched_cmdline" == *"$PROJECT_DIR/proxy.py"* ]]; then
        kill "$proxy_pid" 2>/dev/null || true
      fi
    fi
    return 1
  fi
}

start_systemd_proxy() {
  local unit_ready=0
  if service_matches_project; then
    unit_ready=1
  elif PROXY_PORT="$PROXY_PORT" PYTHON="$PYTHON_BIN" bash "$SCRIPT_DIR/install-wsl-bridge-service.sh" >/dev/null; then
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

install_bundled_claude_science() {
  if [ ! -f "$BUNDLED_CLAUDE_BIN" ]; then
    return 1
  fi
  mkdir -p "$MANAGED_CLAUDE_DIR" || return 1
  cp -f "$BUNDLED_CLAUDE_BIN" "$MANAGED_CLAUDE_BIN" || return 1
  chmod 755 "$MANAGED_CLAUDE_BIN" || return 1
  if [ -f "$BUNDLED_CLAUDE_SHA" ]; then
    (cd "$MANAGED_CLAUDE_DIR" && sha256sum -c "$BUNDLED_CLAUDE_SHA") || return 1
  fi
  return 0
}

if [ -n "${CLAUDE_SCIENCE_BIN:-}" ]; then
  SOURCE_BIN="$CLAUDE_SCIENCE_BIN"
elif [ -x "$MANAGED_CLAUDE_BIN" ]; then
  if [ -f "$BUNDLED_CLAUDE_SHA" ] && ! (cd "$MANAGED_CLAUDE_DIR" && sha256sum -c "$BUNDLED_CLAUDE_SHA" >/dev/null 2>&1); then
    echo "Product-managed Claude Science binary hash does not match the locked bundled version; reinstalling bundled binary."
    install_bundled_claude_science
  fi
  SOURCE_BIN="$MANAGED_CLAUDE_BIN"
elif install_bundled_claude_science; then
  SOURCE_BIN="$MANAGED_CLAUDE_BIN"
else
  SOURCE_BIN="$HOME/.local/bin/claude-science"
fi

if [ ! -x "$SOURCE_BIN" ]; then
  cat >&2 <<EOF
Claude Science Linux binary not found or not executable.
Checked product-managed path: $MANAGED_CLAUDE_BIN
Checked user path: $HOME/.local/bin/claude-science

Use the full portable package that includes vendor/claude-science/linux-x64/claude-science,
then run 1-run-acceptance-preview.bat and 4-install-runtime-after-preview.bat.
EOF
  exit 1
fi

if mkdir -p "$LOG_DIR" 2>/dev/null && : >>"$LOG_FILE" 2>/dev/null; then
  :
else
  echo "Warning: WSL log directory is not writable; proxy fallback logs will be discarded." >&2
  LOG_FILE="/dev/null"
fi
mkdir -p "$PATCH_DIR"

echo "Using Claude Science Linux binary: $SOURCE_BIN"
"$SOURCE_BIN" --version 2>/dev/null || true

if [ "${CSA_FORCE_RESTART:-0}" != "1" ] \
  && check_bridge_health \
  && service_matches_project \
  && pgrep -f "claude-science-api-bridge/patched/claude-science serve" >/dev/null 2>&1; then
  echo "Claude Science and WSL BYOK proxy are already running; using fast start path."
  if [ -x "$PATCHED_BIN" ]; then
    "$PATCHED_BIN" url || true
  else
    echo "http://localhost:$CLAUDE_SCIENCE_PORT"
  fi
  exit 0
fi

if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" = "systemd" ]; then
  start_systemd_proxy || start_fallback_proxy
elif [ "${CSA_FORCE_RESTART:-0}" = "1" ]; then
  stop_stale_bridge_listener
  start_fallback_proxy
elif ! check_bridge_health; then
  start_fallback_proxy
else
  echo "WSL BYOK proxy already healthy on 127.0.0.1:$PROXY_PORT"
fi

TOKEN_FILE="$HOME/.claude-science/.oauth-tokens/byok-user-000000000000000000.enc"
if [ -f "$HOME/.claude-science/encryption.key" ]; then
  if [ -f "$TOKEN_FILE" ]; then
    echo "Local fake OAuth token already exists"
  else
    echo "Refreshing local fake OAuth token"
    "$PYTHON_BIN" "$PROJECT_DIR/setup-token.py" >/dev/null
  fi
else
  echo "Warning: ~/.claude-science/encryption.key does not exist; fake OAuth token was not generated." >&2
fi

SOURCE_SHA="$(sha256sum "$SOURCE_BIN" | awk '{print $1}')"
PATCH_MARKER="$PATCH_DIR/.claude-science.source.sha256"
PATCH_CACHE_KEY="$SOURCE_SHA:$PROXY_PORT"

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
]

for olds, new in pairs:
    for old in olds:
        if len(old) != len(new):
            raise SystemExit(f"length mismatch: {old!r} ({len(old)}) -> {new!r} ({len(new)})")

data = target.read_bytes()
counts = [(olds, new, sum(data.count(old) for old in olds), data.count(new)) for olds, new in pairs]
missing = [
    " or ".join(old.decode() for old in olds)
    for olds, new, old_count, new_count in counts
    if old_count == 0 and new_count == 0
]
if missing:
    raise SystemExit(
        "Unsupported Claude Science daemon build; expected OAuth/API URL(s) not found:\n"
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

print(f"Patched OAuth/API URL occurrence(s): {patched}")
PY

  if ! "$PATCHED_BIN" --help >/dev/null 2>&1; then
    echo "Patched daemon failed executable check." >&2
    exit 1
  fi
  printf '%s\n' "$PATCH_CACHE_KEY" > "$PATCH_MARKER"
fi

for pid in $(pgrep -f "claude-science" 2>/dev/null || true); do
  if [ "$pid" = "$$" ]; then
    continue
  fi
  if [ ! -r "/proc/$pid/cmdline" ]; then
    continue
  fi
  cmd="$(tr '\0' ' ' 2>/dev/null <"/proc/$pid/cmdline" || true)"
  if [[ "$cmd" == *"claude-science serve"* ]]; then
    kill "$pid" 2>/dev/null || true
  fi
done
sleep 1

PROXY_URL="http://127.0.0.1:$PROXY_PORT"
ANTHROPIC_BASE_URL="$PROXY_URL" "$PATCHED_BIN" serve --port "$CLAUDE_SCIENCE_PORT" --no-browser --detached

echo "Started Claude Science patched copy:"
echo "  daemon: $PATCHED_BIN"
echo "  ANTHROPIC_BASE_URL=$PROXY_URL"
"$PATCHED_BIN" url
