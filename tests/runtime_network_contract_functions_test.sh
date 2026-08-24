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
managed_fixture_pid=""
cleanup() {
  if [ -n "$candidate_pid" ]; then
    kill "$candidate_pid" 2>/dev/null || true
    wait "$candidate_pid" 2>/dev/null || true
  fi
  if [ -n "$managed_fixture_pid" ]; then
    kill "$managed_fixture_pid" 2>/dev/null || true
    wait "$managed_fixture_pid" 2>/dev/null || true
  fi
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

# A daemon may have been launched through patched-current before an atomic
# upgrade moves that pointer to the next generation. The lifecycle preflight
# must continue to recognize the old managed executable so it can be stopped
# safely; resolving argv[0] through the new pointer would misclassify it.
original_claude_root="$CSA_CLAUDE_ROOT"
owned_claude_root="$TEST_ROOT/owned-claude"
owned_runtime_a="$owned_claude_root/patched/runtime-a"
owned_runtime_b="$owned_claude_root/patched/runtime-b"
mkdir -p "$owned_runtime_a" "$owned_runtime_b"
cp "$(readlink -f "$(command -v python3)")" "$owned_runtime_a/claude-science"
cp "$(readlink -f "$(command -v python3)")" "$owned_runtime_b/claude-science"
chmod +x "$owned_runtime_a/claude-science" "$owned_runtime_b/claude-science"
cat >"$owned_runtime_a/serve" <<'PY'
import time

while True:
    time.sleep(1)
PY
ln -s "$owned_runtime_a" "$owned_claude_root/patched-current"
(
  cd "$owned_runtime_a"
  exec "$owned_claude_root/patched-current/claude-science" serve
) &
managed_fixture_pid=$!
CSA_CLAUDE_ROOT="$owned_claude_root"
deadline=$((SECONDS + 5))
while [ "$SECONDS" -lt "$deadline" ]; do
  managed_claude_pid "$managed_fixture_pid" && break
  sleep 0.05
done
if ! managed_claude_pid "$managed_fixture_pid"; then
  echo "Lifecycle rejected a managed daemon launched through patched-current." >&2
  exit 1
fi
ln -sfn "$owned_runtime_b" "$owned_claude_root/patched-current"
managed_claude_pid "$managed_fixture_pid" || {
  echo "Lifecycle lost ownership after patched-current advanced generations." >&2
  exit 1
}
kill "$managed_fixture_pid"
wait "$managed_fixture_pid" 2>/dev/null || true
managed_fixture_pid=""
CSA_CLAUDE_ROOT="$original_claude_root"

# The detached daemon must inherit its verified Linux runtime directory, not
# the portable package's /mnt/c or /mnt/e working directory.
launch_fixture_dir="$TEST_ROOT/runtime"
launch_fixture="$launch_fixture_dir/claude-science"
launch_cwd_file="$TEST_ROOT/launch-cwd"
mkdir -p "$launch_fixture_dir"
cat >"$launch_fixture" <<'SH'
#!/usr/bin/env bash
pwd -P >"$CSA_TEST_LAUNCH_CWD_FILE"
printf '%s\n' "$ANTHROPIC_BASE_URL" >"$CSA_TEST_LAUNCH_BASE_URL_FILE"
SH
chmod +x "$launch_fixture"
CSA_TEST_LAUNCH_CWD_FILE="$launch_cwd_file" \
CSA_TEST_LAUNCH_BASE_URL_FILE="$TEST_ROOT/launch-base-url" \
launch_claude_daemon "$launch_fixture" "http://127.0.0.1:9876"
[ "$(cat "$launch_cwd_file")" = "$launch_fixture_dir" ] || {
  echo "Claude Science inherited the caller/package working directory." >&2
  exit 1
}
[ "$(cat "$TEST_ROOT/launch-base-url")" = "http://127.0.0.1:9876" ] || {
  echo "Claude Science launch lost its local Bridge base URL." >&2
  exit 1
}

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

# Local readiness is deliberately observational: it verifies the two listener
# owners, exact executable/argv, and safe Linux thread states without issuing an
# HTTP request that could initialize the application, feature flags, or MCP
# catalog.  Real API reachability is covered separately by the deep SOCKS5H
# contract test below.
primary_fixture="$candidate_pid"
auxiliary_fixture="$candidate_pid"
curl() {
  : >"$TEST_ROOT/unexpected-health-curl"
  return 99
}
if ! check_claude_health "$PATCHED_BIN"; then
  echo "A signalable exact daemon candidate was not accepted as locally ready." >&2
  exit 1
fi
if ( process_threads_signalable() { return 1; }; check_claude_health "$PATCHED_BIN" ); then
  echo "A daemon with an unsafe thread state was incorrectly accepted as locally ready." >&2
  exit 1
fi
[ ! -e "$TEST_ROOT/unexpected-health-curl" ] || {
  echo "Claude local readiness performed a side-effecting HTTP request." >&2
  exit 1
}
health_function="$(declare -f check_claude_health)"
printf '%s\n' "$health_function" | grep -Fq 'process_threads_signalable "$pid"' || {
  echo "Claude local readiness no longer rejects unsafe thread states." >&2
  exit 1
}
if printf '%s\n' "$health_function" | grep -Eq 'curl|https?://'; then
  echo "Claude local readiness contains an HTTP probe." >&2
  exit 1
fi

cleanup_failed_candidate_processes || true
if kill -0 "$candidate_pid" 2>/dev/null; then
  echo "A failed non-listening candidate survived exact-process cleanup." >&2
  exit 1
fi
wait "$candidate_pid" 2>/dev/null || true
candidate_pid=""

# Startup readiness requires two adjacent successful deep probes.  A success
# separated by a daemon-busy sample must reset the streak, and an all-busy
# sequence must remain degraded without claiming readiness.
cat >"$TEST_ROOT/deep-helper.py" <<'PY'
import json
import os
import sys
from pathlib import Path

sequence_path = Path(os.environ["CSA_TEST_DEEP_SEQUENCE_FILE"])
counter_path = Path(os.environ["CSA_TEST_DEEP_COUNTER_FILE"])
sequence = [line.strip() for line in sequence_path.read_text().splitlines() if line.strip()]
index = int(counter_path.read_text() or "0") if counter_path.exists() else 0
counter_path.write_text(str(index + 1))
verdict = sequence[min(index, len(sequence) - 1)]
final_cache = Path(os.environ["CSA_TEST_FINAL_CACHE"])
if final_cache.exists():
    verdict = "transient_cache_exposed"
if "--cache-file" in sys.argv:
    cache_path = Path(sys.argv[sys.argv.index("--cache-file") + 1])
    cache_path.write_text(json.dumps({
        "claude_pid": 101,
        "claude_start_ticks": 1000,
        "sandbox_forwarder_fingerprint": "fixture-forwarders",
        "sandbox_egress_state": "ok" if verdict == "ready" else verdict,
    }))
print(verdict)
PY
NETWORK_QUALITY_HELPER="$TEST_ROOT/deep-helper.py"
NETWORK_CACHE_FILE="$TEST_ROOT/network-quality.json"
PYTHON_BIN="$(command -v python3)"
CSA_TEST_DEEP_SEQUENCE_FILE="$TEST_ROOT/deep-sequence"
CSA_TEST_DEEP_COUNTER_FILE="$TEST_ROOT/deep-counter"
CSA_TEST_FINAL_CACHE="$NETWORK_CACHE_FILE"
export CSA_TEST_DEEP_SEQUENCE_FILE CSA_TEST_DEEP_COUNTER_FILE CSA_TEST_FINAL_CACHE
sleep() { :; }

printf '%s\n' ready egress_daemon_busy ready ready >"$CSA_TEST_DEEP_SEQUENCE_FILE"
: >"$CSA_TEST_DEEP_COUNTER_FILE"
if ! record_deep_network_quality >"$TEST_ROOT/deep-output" 2>&1; then
  echo "Two consecutive deep successes were not accepted." >&2
  exit 1
fi
[ "$(cat "$CSA_TEST_DEEP_COUNTER_FILE")" = 4 ] || {
  echo "A non-consecutive deep success was incorrectly counted." >&2
  exit 1
}
[ "$(grep -c 'passed twice consecutively' "$TEST_ROOT/deep-output")" = 1 ] || {
  echo "Deep readiness success was not reported exactly once." >&2
  exit 1
}
[ -f "$NETWORK_CACHE_FILE" ] || {
  echo "The consecutively validated deep result was not promoted to the final cache." >&2
  exit 1
}
[ ! -e "${NETWORK_CACHE_FILE}.pending.$$" ] || {
  echo "A pending deep-probe cache leaked after validation." >&2
  exit 1
}

printf '%s\n' egress_daemon_mount_io_busy >"$CSA_TEST_DEEP_SEQUENCE_FILE"
: >"$CSA_TEST_DEEP_COUNTER_FILE"
if record_deep_network_quality >"$TEST_ROOT/deep-output-busy" 2>&1; then
  echo "A persistently I/O-blocked daemon was incorrectly reported ready." >&2
  exit 1
fi
grep -Fq 'daemon was kept running' "$TEST_ROOT/deep-output-busy" || {
  echo "Busy-daemon diagnostics did not preserve the running process." >&2
  exit 1
}

printf '%s\n' egress_daemon_busy egress_daemon_busy egress_daemon_busy ready >"$CSA_TEST_DEEP_SEQUENCE_FILE"
: >"$CSA_TEST_DEEP_COUNTER_FILE"
if record_deep_network_quality >"$TEST_ROOT/deep-output-isolated-ready" 2>&1; then
  echo "A final isolated success was incorrectly accepted as a consecutive pair." >&2
  exit 1
fi
[ "$DEEP_NETWORK_VERDICT" = "insufficient_consecutive_successes" ] || {
  echo "An isolated final success produced a misleading ready verdict." >&2
  exit 1
}
python3 - "$NETWORK_CACHE_FILE" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    report = json.load(stream)
assert report["sandbox_egress_state"] != "ok"
PY

# Lifecycle scripts may recommend manual recovery, but must never execute a
# global WSL shutdown or distro termination as an automatic repair step.
if grep -nE '^[[:space:]]*(wsl|wsl\.exe)[[:space:]].*--(shutdown|terminate)' \
  "$PROJECT_SOURCE/scripts/start-claude-science-wsl.sh"; then
  echo "Startup script contains an automatic WSL shutdown/terminate command." >&2
  exit 1
fi

echo "runtime network contract function test passed"
