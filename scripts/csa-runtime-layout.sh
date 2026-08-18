#!/usr/bin/env bash

# Stable, content-addressed runtime layout shared by the installer and launcher.
# This file is sourced by other scripts; do not enable or disable shell options here.

CSA_STATE_ROOT="${CSA_STATE_ROOT:-$HOME/.local/share/csa}"
CSA_LEGACY_STATE_ROOT="${CSA_LEGACY_STATE_ROOT:-$HOME/.local/share/claude-science-api-bridge}"
CSA_BRIDGE_ROOT="$CSA_STATE_ROOT/runtime/bridge"
CSA_CLAUDE_ROOT="$CSA_STATE_ROOT/runtime/claude-science"

csa_sha256() {
  sha256sum "$1" | awk '{print tolower($1)}'
}

csa_valid_sha256() {
  [[ "$1" =~ ^[0-9a-fA-F]{64}$ ]]
}

csa_valid_version() {
  [[ "$1" =~ ^[0-9]+([.][0-9]+)*$ ]]
}

# Prints -1 when A < B, 0 when equal, and 1 when A > B.
csa_compare_versions() {
  local left="$1" right="$2"
  csa_valid_version "$left" && csa_valid_version "$right" || return 2
  awk -v left="$left" -v right="$right" 'BEGIN {
    left_count = split(left, l, ".")
    right_count = split(right, r, ".")
    width = left_count > right_count ? left_count : right_count
    for (i = 1; i <= width; i++) {
      lv = i <= left_count ? l[i] + 0 : 0
      rv = i <= right_count ? r[i] + 0 : 0
      if (lv < rv) { print -1; exit }
      if (lv > rv) { print 1; exit }
    }
    print 0
  }'
}

csa_binary_version() {
  "$1" --version 2>/dev/null \
    | grep -oE '[0-9]+([.][0-9]+)+' \
    | head -n 1
}

csa_json_field() {
  python3 - "$1" "$2" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8-sig") as stream:
    value = json.load(stream)
result = value
for part in sys.argv[2].split("."):
    result = result[part]
print(result)
PY
}

csa_bridge_bundle_sha() {
  local root="$1" file
  for file in proxy.py setup-token.py requirements.txt static/dashboard.html; do
    if [ ! -f "$root/$file" ]; then
      echo "Bridge runtime source is incomplete: $root/$file" >&2
      return 2
    fi
  done
  {
    for file in proxy.py setup-token.py requirements.txt static/dashboard.html; do
      printf '%s  %s\n' "$(csa_sha256 "$root/$file")" "$file"
    done
  } | sha256sum | awk '{print tolower($1)}'
}

csa_atomic_symlink() {
  local target="$1" link_path="$2" parent temporary
  parent="$(dirname "$link_path")"
  temporary="$parent/.$(basename "$link_path").tmp.$$"
  mkdir -p "$parent"
  rm -f "$temporary"
  ln -s "$target" "$temporary"
  mv -Tf "$temporary" "$link_path"
}

csa_pointer_target() {
  local pointer="$1" target
  if [ ! -e "$pointer" ] && [ ! -L "$pointer" ]; then
    return 0
  fi
  target="$(readlink -f "$pointer" 2>/dev/null || true)"
  if [ -n "$target" ] && [ -d "$target" ]; then
    printf '%s\n' "$target"
  fi
}

csa_current_target() {
  csa_pointer_target "$1/current"
}

csa_activate_pointer() {
  local root="$1" candidate="$2" previous
  previous="$(csa_current_target "$root")"
  CSA_POINTER_CHANGED=0
  CSA_PREVIOUS_RUNTIME="$previous"
  if [ "$previous" = "$candidate" ]; then
    return 0
  fi
  if [ -n "$previous" ] && [ -d "$previous" ]; then
    csa_atomic_symlink "$previous" "$root/previous"
  fi
  csa_atomic_symlink "$candidate" "$root/current"
  CSA_POINTER_CHANGED=1
}

csa_restore_previous_pointer() {
  local root="$1" previous current
  previous="$(csa_pointer_target "$root/previous")"
  current="$(csa_current_target "$root")"
  if [ -z "$previous" ] || [ ! -d "$previous" ]; then
    echo "No previous managed runtime is available under $root." >&2
    return 1
  fi
  csa_atomic_symlink "$previous" "$root/current"
  if [ -n "$current" ] && [ -d "$current" ] && [ "$current" != "$previous" ]; then
    csa_atomic_symlink "$current" "$root/previous"
  fi
}

csa_stage_bridge_runtime() {
  local project_dir="$1" package_version="${2:-unknown}"
  local source_sha bundle_sha runtime_id versions candidate temporary file
  local manifest_runtime_id manifest_version manifest_source_sha manifest_bundle_sha candidate_bundle_sha
  project_dir="$(cd "$project_dir" && pwd)"
  for file in proxy.py setup-token.py requirements.txt static/dashboard.html; do
    if [ ! -f "$project_dir/$file" ]; then
      echo "Bridge runtime source is incomplete: $project_dir/$file" >&2
      return 2
    fi
  done
  if ! [[ "$package_version" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "Invalid CSA package version: $package_version" >&2
    return 2
  fi

  source_sha="$(csa_sha256 "$project_dir/proxy.py")"
  bundle_sha="$(csa_bridge_bundle_sha "$project_dir")" || return 1
  runtime_id="bridge-${package_version}-${bundle_sha:0:16}"
  versions="$CSA_BRIDGE_ROOT/versions"
  candidate="$versions/$runtime_id"

  if [ ! -d "$candidate" ]; then
    mkdir -p "$versions"
    temporary="$versions/.candidate-${runtime_id}-$$"
    rm -rf "$temporary"
    mkdir -p "$temporary/static"
    if ! install -m 0644 "$project_dir/proxy.py" "$temporary/proxy.py" \
      || ! install -m 0644 "$project_dir/setup-token.py" "$temporary/setup-token.py" \
      || ! install -m 0644 "$project_dir/requirements.txt" "$temporary/requirements.txt" \
      || ! install -m 0644 "$project_dir/static/dashboard.html" "$temporary/static/dashboard.html"; then
      rm -rf "$temporary"
      return 1
    fi
    python3 - "$temporary/runtime-manifest.json" "$runtime_id" "$package_version" "$source_sha" "$bundle_sha" <<'PY'
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

path, runtime_id, version, source_sha, bundle_sha = sys.argv[1:]
manifest = {
    "schemaVersion": 1,
    "component": "bridge",
    "runtimeId": runtime_id,
    "version": version,
    "sourceSha256": source_sha,
    "bundleSha256": bundle_sha,
    "createdAt": datetime.now(timezone.utc).isoformat(),
}
Path(path).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
PY
    if [ "$(csa_sha256 "$temporary/proxy.py")" != "$source_sha" ]; then
      rm -rf "$temporary"
      echo "Staged Bridge runtime failed its SHA-256 check." >&2
      return 1
    fi
    if ! mv "$temporary" "$candidate" 2>/dev/null; then
      rm -rf "$temporary"
      [ -d "$candidate" ] || return 1
    fi
  fi

  manifest_runtime_id="$(csa_json_field "$candidate/runtime-manifest.json" runtimeId)" || return 1
  manifest_version="$(csa_json_field "$candidate/runtime-manifest.json" version)" || return 1
  manifest_source_sha="$(csa_json_field "$candidate/runtime-manifest.json" sourceSha256 | tr '[:upper:]' '[:lower:]')" || return 1
  manifest_bundle_sha="$(csa_json_field "$candidate/runtime-manifest.json" bundleSha256 | tr '[:upper:]' '[:lower:]')" || return 1
  candidate_bundle_sha="$(csa_bridge_bundle_sha "$candidate")" || return 1
  if [ "$manifest_runtime_id" != "$runtime_id" ] \
    || [ "$manifest_version" != "$package_version" ] \
    || [ "$manifest_source_sha" != "$source_sha" ] \
    || [ "$manifest_bundle_sha" != "$bundle_sha" ] \
    || [ "$candidate_bundle_sha" != "$bundle_sha" ]; then
    echo "Existing managed Bridge candidate failed identity verification: $candidate" >&2
    return 1
  fi

  csa_activate_pointer "$CSA_BRIDGE_ROOT" "$candidate" || return 1
  CSA_BRIDGE_RUNTIME_ID="$runtime_id"
  CSA_BRIDGE_VERSION="$package_version"
  CSA_BRIDGE_SOURCE_SHA256="$source_sha"
  CSA_BRIDGE_RUNTIME_DIR="$(csa_current_target "$CSA_BRIDGE_ROOT")"
  CSA_BRIDGE_MANAGED=1
  export CSA_BRIDGE_RUNTIME_ID CSA_BRIDGE_VERSION CSA_BRIDGE_SOURCE_SHA256 CSA_BRIDGE_RUNTIME_DIR CSA_BRIDGE_MANAGED
}

csa_load_bridge_runtime_identity() {
  local current manifest manifest_component manifest_bundle_sha actual_bundle_sha expected_runtime_id
  current="$(csa_current_target "$CSA_BRIDGE_ROOT")"
  manifest="$current/runtime-manifest.json"
  if [ -z "$current" ] || [ ! -f "$manifest" ] || [ ! -f "$current/proxy.py" ]; then
    echo "Managed Bridge current pointer is missing or incomplete." >&2
    return 1
  fi
  CSA_BRIDGE_RUNTIME_ID="$(csa_json_field "$manifest" runtimeId)"
  CSA_BRIDGE_VERSION="$(csa_json_field "$manifest" version)"
  CSA_BRIDGE_SOURCE_SHA256="$(csa_json_field "$manifest" sourceSha256 | tr '[:upper:]' '[:lower:]')"
  manifest_component="$(csa_json_field "$manifest" component)"
  manifest_bundle_sha="$(csa_json_field "$manifest" bundleSha256 | tr '[:upper:]' '[:lower:]')"
  actual_bundle_sha="$(csa_bridge_bundle_sha "$current")" || return 1
  expected_runtime_id="bridge-${CSA_BRIDGE_VERSION}-${actual_bundle_sha:0:16}"
  CSA_BRIDGE_RUNTIME_DIR="$current"
  CSA_BRIDGE_MANAGED=1
  if [ "$manifest_component" != "bridge" ] \
    || [ "$CSA_BRIDGE_RUNTIME_ID" != "$expected_runtime_id" ] \
    || [ "$(basename "$current")" != "$CSA_BRIDGE_RUNTIME_ID" ] \
    || [ "$manifest_bundle_sha" != "$actual_bundle_sha" ] \
    || [ "$(csa_sha256 "$current/proxy.py")" != "$CSA_BRIDGE_SOURCE_SHA256" ]; then
    echo "Managed Bridge current pointer failed its identity check." >&2
    return 1
  fi
  export CSA_BRIDGE_RUNTIME_ID CSA_BRIDGE_VERSION CSA_BRIDGE_SOURCE_SHA256 CSA_BRIDGE_RUNTIME_DIR CSA_BRIDGE_MANAGED
}

csa_stage_existing_claude_runtime() {
  local source_bin="$1" source_kind="$2" version sha runtime_id versions candidate temporary
  local manifest_component manifest_runtime_id manifest_version manifest_sha candidate_sha
  version="$(csa_binary_version "$source_bin")"
  if ! csa_valid_version "$version"; then
    echo "Unable to determine Claude Science version from $source_bin" >&2
    return 1
  fi
  sha="$(csa_sha256 "$source_bin")"
  runtime_id="${version}-${sha:0:16}"
  versions="$CSA_CLAUDE_ROOT/versions"
  candidate="$versions/$runtime_id"
  if [ ! -d "$candidate" ]; then
    mkdir -p "$versions"
    temporary="$versions/.candidate-${runtime_id}-$$"
    rm -rf "$temporary"
    mkdir -p "$temporary"
    if ! install -m 0755 "$source_bin" "$temporary/claude-science"; then
      rm -rf "$temporary"
      return 1
    fi
    python3 - "$temporary/runtime-manifest.json" "$runtime_id" "$version" "$sha" "$source_kind" <<'PY'
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

path, runtime_id, version, sha, source_kind = sys.argv[1:]
manifest = {
    "schemaVersion": 1,
    "component": "claude-science",
    "runtimeId": runtime_id,
    "version": version,
    "sha256": sha,
    "sourceKind": source_kind,
    "createdAt": datetime.now(timezone.utc).isoformat(),
}
Path(path).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
PY
    if [ "$(csa_sha256 "$temporary/claude-science")" != "$sha" ]; then
      rm -rf "$temporary"
      echo "Staged Claude Science runtime failed its SHA-256 check." >&2
      return 1
    fi
    if ! mv "$temporary" "$candidate" 2>/dev/null; then
      rm -rf "$temporary"
      [ -d "$candidate" ] || return 1
    fi
  fi
  manifest_component="$(csa_json_field "$candidate/runtime-manifest.json" component)" || return 1
  manifest_runtime_id="$(csa_json_field "$candidate/runtime-manifest.json" runtimeId)" || return 1
  manifest_version="$(csa_json_field "$candidate/runtime-manifest.json" version)" || return 1
  manifest_sha="$(csa_json_field "$candidate/runtime-manifest.json" sha256 | tr '[:upper:]' '[:lower:]')" || return 1
  candidate_sha="$(csa_sha256 "$candidate/claude-science")" || return 1
  if [ "$manifest_component" != "claude-science" ] \
    || [ "$manifest_runtime_id" != "$runtime_id" ] \
    || [ "$manifest_version" != "$version" ] \
    || [ "$manifest_sha" != "$sha" ] \
    || [ "$candidate_sha" != "$sha" ]; then
    echo "Existing managed Claude Science candidate failed identity verification: $candidate" >&2
    return 1
  fi
  csa_activate_pointer "$CSA_CLAUDE_ROOT" "$candidate" || return 1
  CSA_CLAUDE_RUNTIME_ID="$runtime_id"
  CSA_CLAUDE_VERSION="$version"
  CSA_CLAUDE_SOURCE_SHA256="$sha"
  CSA_CLAUDE_RUNTIME_DIR="$(csa_current_target "$CSA_CLAUDE_ROOT")"
  export CSA_CLAUDE_RUNTIME_ID CSA_CLAUDE_VERSION CSA_CLAUDE_SOURCE_SHA256 CSA_CLAUDE_RUNTIME_DIR
}

csa_load_claude_runtime_identity() {
  local current manifest manifest_component expected_runtime_id
  current="$(csa_current_target "$CSA_CLAUDE_ROOT")"
  manifest="$current/runtime-manifest.json"
  if [ -z "$current" ] || [ ! -f "$manifest" ] || [ ! -x "$current/claude-science" ]; then
    echo "Managed Claude Science current pointer is missing or incomplete." >&2
    return 1
  fi
  CSA_CLAUDE_RUNTIME_ID="$(csa_json_field "$manifest" runtimeId)"
  CSA_CLAUDE_VERSION="$(csa_json_field "$manifest" version)"
  CSA_CLAUDE_SOURCE_SHA256="$(csa_json_field "$manifest" sha256 | tr '[:upper:]' '[:lower:]')"
  manifest_component="$(csa_json_field "$manifest" component)"
  expected_runtime_id="${CSA_CLAUDE_VERSION}-${CSA_CLAUDE_SOURCE_SHA256:0:16}"
  CSA_CLAUDE_RUNTIME_DIR="$current"
  if [ "$manifest_component" != "claude-science" ] \
    || [ "$CSA_CLAUDE_RUNTIME_ID" != "$expected_runtime_id" ] \
    || [ "$(basename "$current")" != "$CSA_CLAUDE_RUNTIME_ID" ] \
    || [ "$(csa_sha256 "$current/claude-science")" != "$CSA_CLAUDE_SOURCE_SHA256" ]; then
    echo "Managed Claude Science current pointer failed its identity check." >&2
    return 1
  fi
  export CSA_CLAUDE_RUNTIME_ID CSA_CLAUDE_VERSION CSA_CLAUDE_SOURCE_SHA256 CSA_CLAUDE_RUNTIME_DIR
}

csa_active_claude_binary() {
  local current="$CSA_CLAUDE_ROOT/current/claude-science"
  if [ -x "$current" ]; then
    printf '%s\n' "$current"
  elif [ -x "$CSA_LEGACY_STATE_ROOT/bin/claude-science" ]; then
    printf '%s\n' "$CSA_LEGACY_STATE_ROOT/bin/claude-science"
  elif [ -x "$HOME/.local/bin/claude-science" ]; then
    printf '%s\n' "$HOME/.local/bin/claude-science"
  fi
}

csa_backup_before_downgrade() {
  local project_dir="$1" data_dir backup_dir
  data_dir="${CLAUDE_SCIENCE_DATA_DIR:-$HOME/.claude-science}"
  if [ ! -f "$data_dir/operon-cli.db" ]; then
    return 0
  fi
  backup_dir="${CSA_RUNTIME_BACKUP_DIR:-$CSA_STATE_ROOT/backup/claude-science-before-downgrade-$(date -u +%Y%m%dT%H%M%SZ)}"
  if [ ! -x "$project_dir/scripts/backup-claude-science-data.sh" ] \
    && [ ! -f "$project_dir/scripts/backup-claude-science-data.sh" ]; then
    echo "Runtime downgrade requires the data backup helper." >&2
    return 1
  fi
  bash "$project_dir/scripts/backup-claude-science-data.sh" "$data_dir" "$backup_dir"
}

csa_guard_claude_downgrade() {
  local project_dir="$1" candidate_version="$2" active_bin active_version comparison
  active_bin="$(csa_active_claude_binary)"
  if [ -z "$active_bin" ]; then
    return 0
  fi
  active_version="$(csa_binary_version "$active_bin")"
  if ! csa_valid_version "$active_version"; then
    echo "Unable to determine the active Claude Science version." >&2
    return 1
  fi
  comparison="$(csa_compare_versions "$candidate_version" "$active_version")"
  if [ "$comparison" = "-1" ]; then
    if [ "${CSA_ALLOW_RUNTIME_DOWNGRADE:-0}" != "1" ]; then
      echo "Implicit Claude Science downgrade rejected: active=$active_version candidate=$candidate_version." >&2
      return 1
    fi
    csa_backup_before_downgrade "$project_dir" || return 1
  fi
}

csa_stage_claude_runtime() {
  local project_dir="$1" bundled_dir bundled_bin bundled_manifest bundled_version bundled_sha
  local active_bin override_version
  project_dir="$(cd "$project_dir" && pwd)"
  bundled_dir="$project_dir/vendor/claude-science/linux-x64"
  bundled_bin="$bundled_dir/claude-science"
  bundled_manifest="$bundled_dir/manifest.json"
  active_bin="$(csa_active_claude_binary)"

  if [ -n "${CLAUDE_SCIENCE_BIN:-}" ]; then
    if [ ! -x "$CLAUDE_SCIENCE_BIN" ]; then
      echo "CLAUDE_SCIENCE_BIN is not executable: $CLAUDE_SCIENCE_BIN" >&2
      return 1
    fi
    override_version="$(csa_binary_version "$CLAUDE_SCIENCE_BIN")"
    if ! csa_valid_version "$override_version"; then
      echo "Unable to determine Claude Science override version." >&2
      return 1
    fi
    csa_guard_claude_downgrade "$project_dir" "$override_version" || return 1
    csa_stage_existing_claude_runtime "$CLAUDE_SCIENCE_BIN" "override" || return 1
    return
  fi

  if [ ! -f "$bundled_bin" ] || [ ! -f "$bundled_manifest" ]; then
    if [ -z "$active_bin" ]; then
      echo "No bundled or existing Claude Science Linux runtime is available." >&2
      return 1
    fi
    csa_stage_existing_claude_runtime "$active_bin" "existing" || return 1
    return
  fi

  bundled_version="$(csa_json_field "$bundled_manifest" version)"
  bundled_sha="$(csa_json_field "$bundled_manifest" sha256 | tr '[:upper:]' '[:lower:]')"
  if ! csa_valid_version "$bundled_version" || ! csa_valid_sha256 "$bundled_sha"; then
    echo "Bundled Claude Science manifest is invalid." >&2
    return 1
  fi
  if [ "$(csa_sha256 "$bundled_bin")" != "$bundled_sha" ]; then
    echo "Bundled Claude Science hash does not match manifest.json." >&2
    return 1
  fi

  csa_guard_claude_downgrade "$project_dir" "$bundled_version" || return 1

  csa_stage_existing_claude_runtime "$bundled_bin" "bundled" || return 1
  if [ "$CSA_CLAUDE_VERSION" != "$bundled_version" ] \
    || [ "$CSA_CLAUDE_SOURCE_SHA256" != "$bundled_sha" ]; then
    echo "Activated Claude Science runtime does not match the bundled manifest." >&2
    return 1
  fi
}

csa_bridge_identity_json() {
  local runtime_pids runtime_pid
  runtime_pids="$(ss -ltnp "sport = :${PROXY_PORT:-9876}" 2>/dev/null \
    | grep -o 'pid=[0-9]*' \
    | cut -d= -f2 \
    | sort -u)"
  if [ -z "$runtime_pids" ] || [ "$(printf '%s\n' "$runtime_pids" | wc -l)" != "1" ]; then
    echo "Managed Bridge identity requires exactly one listener PID." >&2
    return 1
  fi
  runtime_pid="$runtime_pids"
  python3 - "$CSA_BRIDGE_RUNTIME_ID" "$CSA_BRIDGE_VERSION" \
    "$CSA_BRIDGE_RUNTIME_DIR/proxy.py" "$CSA_BRIDGE_SOURCE_SHA256" "$runtime_pid" <<'PY'
import json
import sys

runtime_id, version, source_path, source_sha, pid = sys.argv[1:]
print(json.dumps({
    "schemaVersion": 1,
    "component": "bridge",
    "runtimeId": runtime_id,
    "version": version,
    "buildId": source_sha[:16],
    "sourcePath": source_path,
    "sourceSha256": source_sha,
    "pid": int(pid),
    "capabilities": ["anthropicBridge", "configRevision", "health"],
    "managed": True,
}, separators=(",", ":")))
PY
}

csa_print_bridge_identity() {
  printf 'CSA_RUNTIME_IDENTITY=%s\n' "$(csa_bridge_identity_json)"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  command="${1:-}"
  case "$command" in
    compare)
      csa_compare_versions "${2:-}" "${3:-}"
      ;;
    stage-bridge)
      csa_stage_bridge_runtime "${2:-}" "${3:-unknown}"
      printf '%s\n' "$CSA_BRIDGE_RUNTIME_DIR"
      ;;
    stage-claude)
      csa_stage_claude_runtime "${2:-}"
      printf '%s\n' "$CSA_CLAUDE_RUNTIME_DIR/claude-science"
      ;;
    restore-bridge)
      csa_restore_previous_pointer "$CSA_BRIDGE_ROOT"
      ;;
    restore-claude)
      csa_restore_previous_pointer "$CSA_CLAUDE_ROOT"
      ;;
    *)
      echo "Usage: csa-runtime-layout.sh compare A B | stage-bridge PROJECT VERSION | stage-claude PROJECT | restore-bridge | restore-claude" >&2
      exit 2
      ;;
  esac
fi
