#!/usr/bin/env bash
set -euo pipefail

PROJECT_SOURCE="${1:-}"
if [ -z "$PROJECT_SOURCE" ]; then
  echo "Usage: runtime_network_contract_functions_test.sh /path/to/csa/source" >&2
  exit 2
fi
PROJECT_SOURCE="$(cd "$PROJECT_SOURCE" && pwd)"

# shellcheck source=../scripts/start-claude-science-wsl.sh
source "$PROJECT_SOURCE/scripts/start-claude-science-wsl.sh"

primary_fixture=""
auxiliary_fixture=""
listener_pids_for_port() {
  case "$1" in
    "$CLAUDE_SCIENCE_PORT") printf '%s\n' "$primary_fixture" | sed '/^$/d';;
    8766) printf '%s\n' "$auxiliary_fixture" | sed '/^$/d';;
    *) return 0;;
  esac
}

primary_fixture=101
auxiliary_fixture=""
if claude_primary_pid >/dev/null; then
  echo "A daemon with only port 8765 was incorrectly accepted." >&2
  exit 1
fi

primary_fixture=101
auxiliary_fixture=202
if claude_primary_pid >/dev/null; then
  echo "Different owners for ports 8765/8766 were incorrectly accepted." >&2
  exit 1
fi

primary_fixture=101
auxiliary_fixture=101
[ "$(claude_primary_pid)" = 101 ] || {
  echo "The same owner on ports 8765/8766 was not accepted." >&2
  exit 1
}

# Exercise rollback cleanup against an exact candidate executable which is
# alive but owns no listening socket.  A copied Python interpreter can execute
# a file named `serve`, giving the same argv shape as the real daemon.
TEST_ROOT="$(mktemp -d)"
candidate_pid=""
cleanup() {
  if [ -n "$candidate_pid" ]; then
    kill "$candidate_pid" 2>/dev/null || true
    wait "$candidate_pid" 2>/dev/null || true
  fi
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

# The published v0.1.5 manifest was emitted by Windows PowerShell with a
# UTF-8 BOM.  Migration must recognize that exact historical package without
# weakening the process/path/manifest ownership checks.
grep -Fq 'encoding="utf-8-sig"' "$PROJECT_SOURCE/scripts/start-claude-science-wsl.sh" || {
  echo "Legacy Bridge verification no longer accepts the published UTF-8 BOM manifest." >&2
  exit 1
}
printf '\357\273\277{"schemaVersion":1}\n' >"$TEST_ROOT/legacy-manifest.json"
python3 - "$TEST_ROOT/legacy-manifest.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8-sig") as handle:
    assert json.load(handle)["schemaVersion"] == 1
PY

python_source="$(command -v python3)"
PATCHED_BIN="$TEST_ROOT/claude-science"
cp "$python_source" "$PATCHED_BIN"
chmod +x "$PATCHED_BIN"
printf 'import time\ntime.sleep(30)\n' >"$TEST_ROOT/serve"
CLAUDE_CANDIDATE_BASELINE_TOKENS="$(exact_executable_serve_tokens "$PATCHED_BIN" || true)"
CLAUDE_CANDIDATE_LAUNCHED=1
(
  cd "$TEST_ROOT"
  exec "$PATCHED_BIN" serve
) &
candidate_pid=$!

deadline=$((SECONDS + 3))
while [ "$SECONDS" -lt "$deadline" ] \
  && ! exact_executable_serve_tokens "$PATCHED_BIN" | grep -q .; do
  sleep 0.05
done
exact_executable_serve_tokens "$PATCHED_BIN" | grep -q . || {
  echo "The non-listening candidate fixture was not discovered." >&2
  exit 1
}

cleanup_failed_candidate_processes || true
if kill -0 "$candidate_pid" 2>/dev/null; then
  echo "A failed non-listening candidate survived exact-process cleanup." >&2
  exit 1
fi
wait "$candidate_pid" 2>/dev/null || true
candidate_pid=""

echo "runtime network contract function test passed"
