#!/usr/bin/env bash
set -euo pipefail

PROJECT_SOURCE="${1:-}"
PYTHON_BIN="${PYTHON:-$HOME/.local/share/claude-science-api-bridge/venv/bin/python}"
TEST_PORT="${CSA_TEST_PORT:-9988}"

if [ -z "$PROJECT_SOURCE" ]; then
  echo "Usage: runtime_activation_integration_test.sh /path/to/csa/source" >&2
  exit 2
fi
PROJECT_SOURCE="$(cd "$PROJECT_SOURCE" && pwd)"
if [ ! -x "$PYTHON_BIN" ]; then
  echo "Bridge test Python is unavailable: $PYTHON_BIN" >&2
  exit 2
fi
if ss -ltn "sport = :$TEST_PORT" 2>/dev/null | grep -q LISTEN; then
  echo "Bridge integration test port is occupied: $TEST_PORT" >&2
  exit 2
fi

TEST_ROOT="$(mktemp -d)"
ORIGINAL_HOME="$HOME"
TEST_HOME="$TEST_ROOT/home"
TEST_PROJECT="$TEST_ROOT/project"
mkdir -p "$TEST_HOME" "$TEST_PROJECT/scripts" "$TEST_PROJECT/static" "$TEST_PROJECT/skills"

for file in proxy.py setup-token.py requirements.txt; do
  cp "$PROJECT_SOURCE/$file" "$TEST_PROJECT/$file"
done
cp "$PROJECT_SOURCE/static/dashboard.html" "$TEST_PROJECT/static/dashboard.html"
for file in csa-runtime-layout.sh start-claude-science-wsl.sh install-wsl-bridge-service.sh; do
  cp "$PROJECT_SOURCE/scripts/$file" "$TEST_PROJECT/scripts/$file"
done

cleanup() {
  local pid cmd
  for pid in $(ss -ltnp "sport = :$TEST_PORT" 2>/dev/null \
    | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u); do
    case "$pid" in ''|*[!0-9]*) continue;; esac
    if [ -r "/proc/$pid/cmdline" ]; then
      cmd="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
      case "$cmd" in *"$TEST_ROOT"*"/proxy.py"*) kill "$pid" 2>/dev/null || true;; esac
    fi
  done
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

export HOME="$TEST_HOME"
export CSA_STATE_ROOT="$TEST_ROOT/state"
export CSA_LEGACY_STATE_ROOT="$TEST_ROOT/legacy"
export CLAUDE_SCIENCE_PROXY_DIR="$TEST_ROOT/config"
export PYTHON="$PYTHON_BIN"
export PROXY_PORT="$TEST_PORT"
export CSA_PACKAGE_VERSION="0.2.0-test"
export CSA_BRIDGE_ONLY=1
export CSA_FORCE_RESTART=1
export CSA_DISABLE_SYSTEMD=1
mkdir -p "$CLAUDE_SCIENCE_PROXY_DIR"
printf '{"proxy_host":"127.0.0.1","proxy_port":%s}\n' "$TEST_PORT" \
  >"$CLAUDE_SCIENCE_PROXY_DIR/config.json"
chmod 600 "$CLAUDE_SCIENCE_PROXY_DIR/config.json"

bash "$TEST_PROJECT/scripts/start-claude-science-wsl.sh" >/dev/null
good_target="$(readlink -f "$CSA_STATE_ROOT/runtime/bridge/current")"
good_runtime_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["runtimeId"])' "$good_target/runtime-manifest.json")"

printf '\nthis is not valid python !!!\n' >>"$TEST_PROJECT/proxy.py"
if bash "$TEST_PROJECT/scripts/start-claude-science-wsl.sh" \
  >"$TEST_ROOT/failure.out" 2>&1; then
  echo "Invalid Bridge candidate unexpectedly passed activation." >&2
  exit 1
fi
grep -q "Managed Bridge candidate failed to start" "$TEST_ROOT/failure.out" \
  || { echo "Candidate failure was not visible." >&2; exit 1; }
grep -q "Restored previous Bridge runtime pointer" "$TEST_ROOT/failure.out" \
  || { echo "Previous Bridge restoration was not reported." >&2; exit 1; }

restored_target="$(readlink -f "$CSA_STATE_ROOT/runtime/bridge/current")"
[ "$restored_target" = "$good_target" ] \
  || { echo "Bridge current pointer did not return to the previous runtime." >&2; exit 1; }
health="$(curl --noproxy '*' -fsS --connect-timeout 1 --max-time 2 \
  "http://127.0.0.1:$TEST_PORT/health")"
python3 - "$health" "$good_runtime_id" <<'PY'
import json
import sys

health = json.loads(sys.argv[1])
identity = health.get("runtime_identity") or {}
assert health.get("status") == "ok"
assert identity.get("managed") is True
assert identity.get("runtimeId") == sys.argv[2]
assert isinstance(identity.get("pid"), int) and identity["pid"] > 0
PY

export HOME="$ORIGINAL_HOME"
echo "runtime activation integration test passed"
