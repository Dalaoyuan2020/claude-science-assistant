#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="${1:?usage: inspect_host_access_integration_test.sh PROJECT_DIR}"
INSPECT="$PROJECT_DIR/skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh"
TEST_ROOT="$(mktemp -d)"
daemon_pid=""
cleanup() {
  if [ -n "$daemon_pid" ]; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

run_case() {
  local name="$1"
  local fixture="$2"
  local expected="$3"
  local home_dir="$TEST_ROOT/$name"
  local output
  mkdir -p "$home_dir/.claude-science"
  printf '%s\n' "$fixture" > "$home_dir/.claude-science/preferences.json"
  output="$(HOME="$home_dir" bash "$INSPECT" "$PROJECT_DIR")"
  printf '%s' "$output" | python3 -c '
import json
import sys

expected = json.loads(sys.argv[1])
report = json.load(sys.stdin)["host_access"]
for key, value in expected.items():
    actual = report.get(key)
    if actual != value:
        raise SystemExit(f"{key}: expected {value!r}, got {actual!r}; report={report!r}")
' "$expected"
}

run_case modern_migrated '
{
  "_migratedToApprovalGrants": true,
  "approvalGrants": {
    "always": {"allow": {"host": ["rw:/mnt/e/project", "rw:/home/test/ext4"]}},
    "project": {"allow": {"host": ["rw:/mnt/e/project-only"]}}
  },
  "hostGrants": [{"path": "/mnt/e/Downloads", "mode": "rw", "createdAt": 0}]
}' '{
  "preferences_parse_ok": true,
  "write_grant_count": 2,
  "drvfs_write_grant_count": 1,
  "drvfs_write_grants": ["rw:/mnt/e/project"],
  "broad_drvfs_write_grant_count": 0
}'

run_case legacy_union '
{
  "_migratedToApprovalGrants": false,
  "approvalGrants": [
    {"kind": "host", "key": "rw:/mnt/E/Documents", "createdAt": "2026-01-01T00:00:00Z"},
    {"kind": "network", "key": "example.org", "createdAt": "2026-01-01T00:00:00Z"}
  ],
  "hostGrants": [
    {"path": "/mnt/E/Documents", "mode": "rw", "createdAt": 0},
    {"path": "/mnt/e/future", "mode": "rw", "createdAt": 0, "expiresAt": 4102444800000},
    {"path": "/mnt/e/expired", "mode": "rw", "createdAt": 0, "expiresAt": 1},
    {"path": "/mnt/e/read-only", "mode": "ro", "createdAt": 0}
  ]
}' '{
  "preferences_parse_ok": true,
  "write_grant_count": 2,
  "drvfs_write_grant_count": 2,
  "drvfs_write_grants": ["rw:/mnt/E/Documents", "rw:/mnt/e/future"],
  "broad_drvfs_write_grant_count": 1,
  "broad_drvfs_write_grants": ["rw:/mnt/E/Documents"]
}'

run_case project_only '
{
  "approvalGrants": {
    "always": {"allow": {}},
    "project": {"allow": {"host": ["rw:/mnt/e/project-only"]}}
  },
  "hostGrants": []
}' '{
  "preferences_parse_ok": true,
  "write_grant_count": 0,
  "drvfs_write_grant_count": 0,
  "drvfs_write_grants": []
}'

run_case malformed '
{"approvalGrants": [null]}
' '{
  "preferences_parse_ok": false,
  "write_grant_count": 0,
  "drvfs_write_grant_count": 0,
  "drvfs_write_grants": [],
  "broad_drvfs_write_grant_count": 0,
  "broad_drvfs_write_grants": []
}'

# A daemon launched through the stable patched-current symlink still resolves
# to a content-addressed managed executable. Inspection must validate the two
# identities without re-resolving a pointer that may advance after launch;
# otherwise a healthy old generation is mislabeled "not started" after upgrade.
owner_home="$TEST_ROOT/runtime-owner-home"
owner_state="$TEST_ROOT/runtime-owner-state"
owner_runtime_a="$owner_state/runtime/claude-science/patched/fixture-runtime-a"
owner_runtime_b="$owner_state/runtime/claude-science/patched/fixture-runtime-b"
stable_runtime="$owner_state/runtime/claude-science/patched-current"
fake_bin="$TEST_ROOT/fake-bin"
mkdir -p "$owner_home" "$owner_runtime_a" "$owner_runtime_b" "$fake_bin"
cp "$(readlink -f "$(command -v python3)")" "$owner_runtime_a/claude-science"
cp "$(readlink -f "$(command -v python3)")" "$owner_runtime_b/claude-science"
chmod +x "$owner_runtime_a/claude-science" "$owner_runtime_b/claude-science"
ln -s "$owner_runtime_a" "$stable_runtime"
cat >"$owner_runtime_a/serve" <<'PY'
import time

while True:
    time.sleep(1)
PY
cat >"$fake_bin/ss" <<'SH'
#!/usr/bin/env bash
case "$*" in
  *"sport = :8765"*) port=8765;;
  *"sport = :8766"*) port=8766;;
  -ltn)
    printf 'LISTEN 0 128 127.0.0.1:8765 0.0.0.0:*\n'
    printf 'LISTEN 0 128 127.0.0.1:8766 0.0.0.0:*\n'
    exit 0
    ;;
  *) exit 0;;
esac
if [[ "$*" == *-ltnp* ]]; then
  printf 'LISTEN 0 128 127.0.0.1:%s 0.0.0.0:* users:(("claude-science",pid=%s,fd=3))\n' \
    "$port" "$CSA_TEST_DAEMON_PID"
else
  printf 'LISTEN 0 128 127.0.0.1:%s 0.0.0.0:*\n' "$port"
fi
SH
chmod +x "$fake_bin/ss"
(
  cd "$owner_runtime_a"
  exec "$stable_runtime/claude-science" serve
) &
daemon_pid=$!
deadline=$((SECONDS + 5))
while [ "$SECONDS" -lt "$deadline" ]; do
  kill -0 "$daemon_pid" 2>/dev/null && break
  sleep 0.05
done

# Simulate an atomic runtime upgrade after launch. argv[0] retains the stable
# pointer string while /proc/PID/exe remains runtime A and patched-current now
# targets B. Both inspection and lifecycle ownership checks must still agree
# that this is the old, managed daemon generation.
ln -sfn "$owner_runtime_b" "$stable_runtime"

runtime_report="$(
  PATH="$fake_bin:$PATH" \
  CSA_TEST_DAEMON_PID="$daemon_pid" \
  HOME="$owner_home" \
  CSA_STATE_ROOT="$owner_state" \
    bash "$INSPECT" "$PROJECT_DIR"
)"
printf '%s' "$runtime_report" | python3 -c '
import json
import sys

runtime = json.load(sys.stdin)["runtime"]
expected_pid = int(sys.argv[1])
assert runtime["claude_owner_verified"] is True, runtime
assert runtime["claude_pid"] == expected_pid, runtime
assert runtime["claude_unverified_pid"] is None, runtime
assert runtime["port_8765"] is True, runtime
assert runtime["port_8766"] is True, runtime
' "$daemon_pid"
kill "$daemon_pid"
wait "$daemon_pid" 2>/dev/null || true
daemon_pid=""

echo "inspect host-access integration tests passed"
