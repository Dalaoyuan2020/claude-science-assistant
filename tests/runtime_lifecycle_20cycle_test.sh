#!/usr/bin/env bash
set -euo pipefail

PROJECT_SOURCE="${1:-}"
PYTHON_BIN="${PYTHON:-$HOME/.local/share/claude-science-api-bridge/venv/bin/python}"
TEST_PORT="${CSA_TEST_PORT:-9987}"

if [ -z "$PROJECT_SOURCE" ]; then
  echo "Usage: runtime_lifecycle_20cycle_test.sh /path/to/csa/source" >&2
  exit 2
fi
PROJECT_SOURCE="$(cd "$PROJECT_SOURCE" && pwd)"
if [ ! -x "$PYTHON_BIN" ]; then
  echo "Bridge test Python is unavailable: $PYTHON_BIN" >&2
  exit 2
fi
if ss -ltn "sport = :$TEST_PORT" 2>/dev/null | grep -q LISTEN; then
  echo "Lifecycle test port is occupied: $TEST_PORT" >&2
  exit 2
fi

TEST_ROOT="$(mktemp -d)"
ORIGINAL_HOME="$HOME"
TEST_HOME="$TEST_ROOT/home"
TEST_PROJECT="$TEST_ROOT/project"
mkdir -p "$TEST_HOME" "$TEST_PROJECT/scripts" "$TEST_PROJECT/static"

for file in proxy.py setup-token.py requirements.txt; do
  cp "$PROJECT_SOURCE/$file" "$TEST_PROJECT/$file"
done
cp "$PROJECT_SOURCE/static/dashboard.html" "$TEST_PROJECT/static/dashboard.html"
for file in csa-runtime-layout.sh csa-network-quality.py start-claude-science-wsl.sh install-wsl-bridge-service.sh; do
  cp "$PROJECT_SOURCE/scripts/$file" "$TEST_PROJECT/scripts/$file"
done

listener_pid() {
  ss -ltnp "sport = :$TEST_PORT" 2>/dev/null \
    | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u
}

stop_test_bridge() {
  local pid cmd deadline
  for pid in $(listener_pid || true); do
    case "$pid" in ''|*[!0-9]*) continue;; esac
    cmd="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
    case "$cmd" in *"$TEST_ROOT"*"/proxy.py"*) kill "$pid" 2>/dev/null || true;; esac
  done
  deadline=$((SECONDS + 4))
  while [ "$SECONDS" -lt "$deadline" ] && [ -n "$(listener_pid || true)" ]; do
    sleep 0.1
  done
  [ -z "$(listener_pid || true)" ] || {
    echo "Test Bridge did not release port $TEST_PORT." >&2
    return 1
  }
}

cleanup() {
  stop_test_bridge >/dev/null 2>&1 || true
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

export HOME="$TEST_HOME"
export CSA_STATE_ROOT="$TEST_ROOT/state"
export CSA_LEGACY_STATE_ROOT="$TEST_ROOT/legacy"
export CLAUDE_SCIENCE_PROXY_DIR="$TEST_ROOT/config"
export PYTHON="$PYTHON_BIN"
export PROXY_PORT="$TEST_PORT"
export CSA_PACKAGE_VERSION="0.1.5-cycle-test"
export CSA_BRIDGE_ONLY=1
export CSA_DISABLE_SYSTEMD=1
mkdir -p "$CLAUDE_SCIENCE_PROXY_DIR"
printf '{"proxy_host":"127.0.0.1","proxy_port":%s}\n' "$TEST_PORT" \
  >"$CLAUDE_SCIENCE_PROXY_DIR/config.json"
chmod 600 "$CLAUDE_SCIENCE_PROXY_DIR/config.json"

assert_single_healthy_owner() {
  local expected_pid="$1" pids health
  pids="$(listener_pid || true)"
  [ "$(printf '%s\n' "$pids" | sed '/^$/d' | wc -l)" = "1" ] || {
    echo "Expected exactly one listener on $TEST_PORT; got: $pids" >&2
    return 1
  }
  [ "$pids" = "$expected_pid" ] || {
    echo "Listener PID mismatch: expected $expected_pid, got $pids" >&2
    return 1
  }
  health="$(curl --noproxy '*' -fsS --connect-timeout 1 --max-time 2 \
    "http://127.0.0.1:$TEST_PORT/health")"
  "$PYTHON_BIN" - "$health" "$expected_pid" <<'PY'
import json, sys
health = json.loads(sys.argv[1])
identity = health.get("runtime_identity") or {}
assert health.get("status") == "ok"
assert identity.get("managed") is True
assert identity.get("component") == "bridge"
assert identity.get("pid") == int(sys.argv[2])
assert len(str(identity.get("sourceSha256") or "")) == 64
PY
}

# Two independent entrypoints must converge on one listener through the WSL
# lifecycle lock; both may return success, but they may not create two owners.
CSA_FORCE_RESTART=0 bash "$TEST_PROJECT/scripts/start-claude-science-wsl.sh" >/dev/null &
concurrent_a=$!
CSA_FORCE_RESTART=0 bash "$TEST_PROJECT/scripts/start-claude-science-wsl.sh" >/dev/null &
concurrent_b=$!
wait "$concurrent_a"
wait "$concurrent_b"
concurrent_owner="$(listener_pid)"
assert_single_healthy_owner "$concurrent_owner"
stop_test_bridge

for cycle in $(seq 1 20); do
  CSA_FORCE_RESTART=0 bash "$TEST_PROJECT/scripts/start-claude-science-wsl.sh" >/dev/null
  first_pid="$(listener_pid)"
  assert_single_healthy_owner "$first_pid"

  CSA_FORCE_RESTART=0 bash "$TEST_PROJECT/scripts/start-claude-science-wsl.sh" >/dev/null
  second_pid="$(listener_pid)"
  [ "$second_pid" = "$first_pid" ] || {
    echo "Cycle $cycle idempotent start replaced PID $first_pid with $second_pid." >&2
    exit 1
  }
  assert_single_healthy_owner "$second_pid"

  CSA_FORCE_RESTART=1 bash "$TEST_PROJECT/scripts/start-claude-science-wsl.sh" >/dev/null
  restarted_pid="$(listener_pid)"
  [ "$restarted_pid" != "$first_pid" ] || {
    echo "Cycle $cycle force restart did not hand off the listener PID." >&2
    exit 1
  }
  assert_single_healthy_owner "$restarted_pid"

  stop_test_bridge
  if find "$CSA_STATE_ROOT/runtime" -name '.candidate-*' -o -name '.current.tmp.*' | grep -q .; then
    echo "Cycle $cycle left a candidate or temporary pointer behind." >&2
    exit 1
  fi
  if ps -eo stat=,args= | awk -v root="$TEST_ROOT" '$0 ~ root && $1 ~ /^Z/ {found=1} END {exit found ? 0 : 1}'; then
    echo "Cycle $cycle left a zombie test process." >&2
    exit 1
  fi
done

export HOME="$ORIGINAL_HOME"
echo "20-cycle runtime lifecycle test passed"
