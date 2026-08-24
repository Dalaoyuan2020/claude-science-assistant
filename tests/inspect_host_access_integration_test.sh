#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="${1:?usage: inspect_host_access_integration_test.sh PROJECT_DIR}"
INSPECT="$PROJECT_DIR/skills/bootstrap-claude-science-wsl/scripts/inspect-wsl.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

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

echo "inspect host-access integration tests passed"
