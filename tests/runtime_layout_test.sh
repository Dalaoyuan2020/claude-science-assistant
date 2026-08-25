#!/usr/bin/env bash
set -euo pipefail

PROJECT_SOURCE="${1:-}"
if [ -z "$PROJECT_SOURCE" ]; then
  echo "Usage: runtime_layout_test.sh /path/to/csa/source" >&2
  exit 2
fi
PROJECT_SOURCE="$(cd "$PROJECT_SOURCE" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

export HOME="$TEST_ROOT/home"
export CSA_STATE_ROOT="$TEST_ROOT/state"
export CSA_LEGACY_STATE_ROOT="$TEST_ROOT/legacy"
mkdir -p "$HOME" "$TEST_ROOT/project/static" "$TEST_ROOT/project/scripts"

cp "$PROJECT_SOURCE/scripts/csa-runtime-layout.sh" "$TEST_ROOT/project/scripts/"
cp "$PROJECT_SOURCE/scripts/backup-claude-science-data.sh" "$TEST_ROOT/project/scripts/"
printf 'print("bridge one")\n' >"$TEST_ROOT/project/proxy.py"
printf 'print("token")\n' >"$TEST_ROOT/project/setup-token.py"
printf 'fastapi\n' >"$TEST_ROOT/project/requirements.txt"
printf '<html>one</html>\n' >"$TEST_ROOT/project/static/dashboard.html"

# shellcheck source=/dev/null
source "$TEST_ROOT/project/scripts/csa-runtime-layout.sh"

fail() {
  echo "runtime layout test failed: $*" >&2
  exit 1
}

start_script="$PROJECT_SOURCE/scripts/start-claude-science-wsl.sh"
host_grant_repair_helper="$PROJECT_SOURCE/scripts/csa-narrow-broad-host-grants.py"
[ -f "$start_script" ] || fail "WSL start script is missing from the runtime source"
[ -f "$host_grant_repair_helper" ] \
  || fail "ForceRestart host-grant repair helper is missing from the runtime source"
grep -Fq 'HOST_GRANT_REPAIR_HELPER="$PROJECT_DIR/scripts/csa-narrow-broad-host-grants.py"' "$start_script" \
  || fail "WSL start script no longer binds ForceRestart to the packaged host-grant repair helper"

assert_eq() {
  [ "$1" = "$2" ] || fail "expected '$2', got '$1'"
}

assert_eq "$(csa_compare_versions 0.1.25 0.1.21)" "1"
assert_eq "$(csa_compare_versions 0.1.21 0.1.25)" "-1"
assert_eq "$(csa_compare_versions 0.1.25 0.1.25.0)" "0"

csa_stage_bridge_runtime "$TEST_ROOT/project" "0.2.0-restart"
first_bridge="$(csa_current_target "$CSA_BRIDGE_ROOT")"
[ -f "$first_bridge/runtime-manifest.json" ] || fail "Bridge manifest was not staged"
[ -z "$CSA_PREVIOUS_RUNTIME" ] || fail "first Bridge activation invented a previous runtime"
[ ! -e "$CSA_BRIDGE_ROOT/previous" ] || fail "first Bridge activation created a previous pointer"
case "$(basename "$first_bridge")" in
  bridge-0.2.0-restart-*) ;;
  *) fail "Bridge runtime ID does not bind the package version" ;;
esac
assert_eq "$(csa_json_field "$first_bridge/runtime-manifest.json" component)" "bridge"

csa_stage_bridge_runtime "$TEST_ROOT/project" "0.2.1"
same_content_new_version="$(csa_current_target "$CSA_BRIDGE_ROOT")"
[ "$same_content_new_version" != "$first_bridge" ] || fail "Bridge package version was not part of candidate identity"
csa_restore_previous_pointer "$CSA_BRIDGE_ROOT"
assert_eq "$(csa_current_target "$CSA_BRIDGE_ROOT")" "$first_bridge"

cp "$first_bridge/requirements.txt" "$TEST_ROOT/requirements.saved"
printf 'tampered\n' >>"$first_bridge/requirements.txt"
if csa_stage_bridge_runtime "$TEST_ROOT/project" "0.2.0-restart" 2>"$TEST_ROOT/bridge-tamper.err"; then
  fail "tampered Bridge candidate was accepted"
fi
grep -q "identity verification" "$TEST_ROOT/bridge-tamper.err" || fail "Bridge tamper failure was not visible"
cp "$TEST_ROOT/requirements.saved" "$first_bridge/requirements.txt"
csa_stage_bridge_runtime "$TEST_ROOT/project" "0.2.0-restart"

printf 'print("bridge two")\n' >"$TEST_ROOT/project/proxy.py"
csa_stage_bridge_runtime "$TEST_ROOT/project" "0.2.0-restart"
second_bridge="$(csa_current_target "$CSA_BRIDGE_ROOT")"
[ "$first_bridge" != "$second_bridge" ] || fail "Bridge content change did not create a candidate"
assert_eq "$(readlink -f "$CSA_BRIDGE_ROOT/previous")" "$first_bridge"
csa_restore_previous_pointer "$CSA_BRIDGE_ROOT"
assert_eq "$(csa_current_target "$CSA_BRIDGE_ROOT")" "$first_bridge"

# Simulate a candidate that passed staging but then failed its health gate.
printf 'print("bridge health failure")\n' >"$TEST_ROOT/project/proxy.py"
csa_stage_bridge_runtime "$TEST_ROOT/project" "0.2.0-restart"
failed_bridge="$(csa_current_target "$CSA_BRIDGE_ROOT")"
[ "$failed_bridge" != "$first_bridge" ] || fail "failure candidate was not staged"
csa_restore_previous_pointer "$CSA_BRIDGE_ROOT"
assert_eq "$(csa_current_target "$CSA_BRIDGE_ROOT")" "$first_bridge"

# A missing source file and an unusable state path must fail before activation.
mv "$TEST_ROOT/project/static/dashboard.html" "$TEST_ROOT/project/static/dashboard.saved"
if csa_stage_bridge_runtime "$TEST_ROOT/project" "0.2.0-restart" 2>"$TEST_ROOT/missing.err"; then
  fail "incomplete Bridge source was accepted"
fi
grep -q "source is incomplete" "$TEST_ROOT/missing.err" || fail "missing-file failure was not visible"
assert_eq "$(csa_current_target "$CSA_BRIDGE_ROOT")" "$first_bridge"
mv "$TEST_ROOT/project/static/dashboard.saved" "$TEST_ROOT/project/static/dashboard.html"

saved_bridge_root="$CSA_BRIDGE_ROOT"
printf 'not a directory\n' >"$TEST_ROOT/blocked-state"
CSA_BRIDGE_ROOT="$TEST_ROOT/blocked-state/bridge"
if csa_stage_bridge_runtime "$TEST_ROOT/project" "0.2.0-restart" 2>"$TEST_ROOT/state.err"; then
  fail "unusable runtime state path was accepted"
fi
[ -s "$TEST_ROOT/state.err" ] || fail "unusable state path failed silently"
CSA_BRIDGE_ROOT="$saved_bridge_root"

mkdir -p "$TEST_ROOT/project/vendor/claude-science/linux-x64"
cat >"$TEST_ROOT/project/vendor/claude-science/linux-x64/claude-science" <<'SH'
#!/usr/bin/env bash
echo 'claude-science 0.1.25 (test)'
SH
chmod +x "$TEST_ROOT/project/vendor/claude-science/linux-x64/claude-science"
candidate_sha="$(csa_sha256 "$TEST_ROOT/project/vendor/claude-science/linux-x64/claude-science")"
python3 - "$TEST_ROOT/project/vendor/claude-science/linux-x64/manifest.json" "$candidate_sha" <<'PY'
import json
import sys
from pathlib import Path

Path(sys.argv[1]).write_text(json.dumps({
    "version": "0.1.25",
    "sha256": sys.argv[2],
}) + "\n", encoding="utf-8-sig")
PY

mkdir -p "$HOME/.local/bin"
cat >"$HOME/.local/bin/claude-science" <<'SH'
#!/usr/bin/env bash
echo 'claude-science 0.1.30 (test)'
SH
chmod +x "$HOME/.local/bin/claude-science"
if csa_stage_claude_runtime "$TEST_ROOT/project" 2>"$TEST_ROOT/downgrade.err"; then
  fail "implicit downgrade was accepted"
fi
grep -q "Implicit Claude Science downgrade rejected" "$TEST_ROOT/downgrade.err" \
  || fail "downgrade rejection was not explicit"
[ ! -e "$CSA_CLAUDE_ROOT/current" ] || fail "rejected downgrade changed current pointer"

rm "$HOME/.local/bin/claude-science"
csa_stage_claude_runtime "$TEST_ROOT/project"
active_claude="$(csa_current_target "$CSA_CLAUDE_ROOT")"
[ -x "$active_claude/claude-science" ] || fail "Claude Science candidate was not activated"
assert_eq "$(csa_json_field "$active_claude/runtime-manifest.json" version)" "0.1.25"

cp "$active_claude/claude-science" "$TEST_ROOT/claude.saved"
printf '\n# tampered\n' >>"$active_claude/claude-science"
if csa_stage_existing_claude_runtime \
  "$TEST_ROOT/project/vendor/claude-science/linux-x64/claude-science" \
  "bundled" 2>"$TEST_ROOT/claude-tamper.err"; then
  fail "tampered Claude Science candidate was accepted"
fi
grep -q "identity verification" "$TEST_ROOT/claude-tamper.err" || fail "Claude tamper failure was not visible"
cp "$TEST_ROOT/claude.saved" "$active_claude/claude-science"
chmod +x "$active_claude/claude-science"

python3 - "$TEST_ROOT/project/vendor/claude-science/linux-x64/manifest.json" <<'PY'
import json
import sys
from pathlib import Path

Path(sys.argv[1]).write_text(json.dumps({
    "version": "0.1.26",
    "sha256": "0" * 64,
}) + "\n", encoding="utf-8-sig")
PY
if csa_stage_claude_runtime "$TEST_ROOT/project" 2>"$TEST_ROOT/hash.err"; then
  fail "invalid candidate hash was accepted"
fi
grep -q "hash does not match" "$TEST_ROOT/hash.err" || fail "hash failure was not visible"
assert_eq "$(csa_current_target "$CSA_CLAUDE_ROOT")" "$active_claude"

echo "runtime layout tests passed"
