#!/usr/bin/env bash
set -euo pipefail

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

if [ -x "$HOME/.local/share/claude-science-api-bridge/venv/bin/python" ]; then
  PYTHON_BIN="${PYTHON:-$HOME/.local/share/claude-science-api-bridge/venv/bin/python}"
else
  PYTHON_BIN="${PYTHON:-python3}"
fi

check_tcp() {
  local host="$1"
  local port="$2"
  python3 - "$host" "$port" <<'PY'
import socket
import sys

host, port = sys.argv[1], int(sys.argv[2])
s = socket.socket()
s.settimeout(0.5)
try:
    ok = s.connect_ex((host, port)) == 0
finally:
    s.close()
raise SystemExit(0 if ok else 1)
PY
}

wait_tcp() {
  local host="$1"
  local port="$2"
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if check_tcp "$host" "$port"; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

if [ "${#PROXY_PORT}" -ne 4 ]; then
  echo "PROXY_PORT must be four digits for byte-length-preserving URL patches. Current: $PROXY_PORT" >&2
  exit 1
fi

install_bundled_claude_science() {
  if [ ! -f "$BUNDLED_CLAUDE_BIN" ]; then
    return 1
  fi
  mkdir -p "$MANAGED_CLAUDE_DIR"
  cp -f "$BUNDLED_CLAUDE_BIN" "$MANAGED_CLAUDE_BIN"
  chmod 755 "$MANAGED_CLAUDE_BIN"
  if [ -f "$BUNDLED_CLAUDE_SHA" ]; then
    (cd "$MANAGED_CLAUDE_DIR" && sha256sum -c "$BUNDLED_CLAUDE_SHA")
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

mkdir -p "$LOG_DIR" "$PATCH_DIR"

echo "Using Claude Science Linux binary: $SOURCE_BIN"
"$SOURCE_BIN" --version 2>/dev/null || true

if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" = "systemd" ]; then
  PROXY_PORT="$PROXY_PORT" PYTHON="$PYTHON_BIN" bash "$SCRIPT_DIR/install-wsl-bridge-service.sh" >/dev/null
  if ! systemctl --user is-active --quiet claude-science-bridge.service; then
    # Remove only an exact legacy Bridge process before handing ownership to systemd.
    for pid in $(ps -eo pid=,args= | awk '/python.*claude-science-api-bridge.*\/proxy.py/ && !/awk/ {print $1}'); do
      kill "$pid" 2>/dev/null || true
    done
    systemctl --user start claude-science-bridge.service
  fi
  echo "WSL BYOK proxy managed by systemd user service on 127.0.0.1:$PROXY_PORT"
  if ! wait_tcp 127.0.0.1 "$PROXY_PORT"; then
    echo "WSL proxy service did not start on 127.0.0.1:$PROXY_PORT. Last log lines:" >&2
    tail -80 "$LOG_DIR/wsl-proxy.log" >&2 || true
    exit 1
  fi
elif ! check_tcp 127.0.0.1 "$PROXY_PORT"; then
  echo "Starting fallback WSL BYOK proxy on 127.0.0.1:$PROXY_PORT"
  (
    cd "$PROJECT_DIR"
    setsid -f "$PYTHON_BIN" "$PROJECT_DIR/proxy.py" >"$LOG_DIR/wsl-proxy.log" 2>&1
  )
  if ! wait_tcp 127.0.0.1 "$PROXY_PORT"; then
    echo "Fallback WSL proxy did not start on 127.0.0.1:$PROXY_PORT. Last log lines:" >&2
    tail -80 "$LOG_DIR/wsl-proxy.log" >&2 || true
    exit 1
  fi
else
  echo "WSL BYOK proxy already listening on 127.0.0.1:$PROXY_PORT"
fi

if [ -f "$HOME/.claude-science/encryption.key" ]; then
  echo "Refreshing local fake OAuth token"
  "$PYTHON_BIN" "$PROJECT_DIR/setup-token.py" >/dev/null
else
  echo "Warning: ~/.claude-science/encryption.key does not exist; fake OAuth token was not generated." >&2
fi

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
