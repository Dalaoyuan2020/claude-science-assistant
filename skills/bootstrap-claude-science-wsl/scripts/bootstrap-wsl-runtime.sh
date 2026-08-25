#!/usr/bin/env bash
set -euo pipefail

# Prepare the WSL-side runtime used by Claude Science Assistant.
#
# Mutating actions are intentionally kept here, behind the Windows-side
# repair-approved.ps1 confirmation gate. Set DRY_RUN=1 to print the plan.

PROJECT_DIR="${1:-}"
PROXY_PORT="${PROXY_PORT:-9876}"
START_SERVICES="${START_SERVICES:-0}"
DRY_RUN="${DRY_RUN:-0}"
LEGACY_STATE_DIR="$HOME/.local/share/claude-science-api-bridge"
VENV_DIR="$LEGACY_STATE_DIR/venv"
PYTHON_BIN="$VENV_DIR/bin/python"

if [ -z "$PROJECT_DIR" ]; then
  echo "Usage: bootstrap-wsl-runtime.sh /path/to/claude-science-api-bridge" >&2
  exit 2
fi

PROJECT_DIR="$(cd "$PROJECT_DIR" && pwd)"
if [ ! -f "$PROJECT_DIR/proxy.py" ] || [ ! -f "$PROJECT_DIR/requirements.txt" ]; then
  echo "Project root is invalid: $PROJECT_DIR" >&2
  exit 2
fi
RUNTIME_LAYOUT_SCRIPT="$PROJECT_DIR/scripts/csa-runtime-layout.sh"
if [ ! -f "$RUNTIME_LAYOUT_SCRIPT" ]; then
  echo "CSA runtime layout helper is missing: $RUNTIME_LAYOUT_SCRIPT" >&2
  exit 2
fi
# shellcheck source=csa-runtime-layout.sh
source "$RUNTIME_LAYOUT_SCRIPT"

say() { printf '%s\n' "$*"; }

run() {
  if [ "$DRY_RUN" = "1" ]; then
    printf '+'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

need_packages=()
if ! command -v python3 >/dev/null 2>&1; then
  need_packages+=(python3)
fi
if ! python3 -m venv --help >/dev/null 2>&1; then
  need_packages+=(python3-venv python3-pip)
fi
if ! command -v curl >/dev/null 2>&1; then
  need_packages+=(curl)
fi
if ! command -v ss >/dev/null 2>&1; then
  need_packages+=(iproute2)
fi

if [ "${#need_packages[@]}" -gt 0 ]; then
  if ! command -v apt-get >/dev/null 2>&1; then
    echo "Missing packages: ${need_packages[*]}; apt-get is unavailable." >&2
    exit 3
  fi
  if [ "$DRY_RUN" = "1" ]; then
    say "+ sudo apt-get update"
    say "+ sudo apt-get install -y ${need_packages[*]}"
  elif sudo -n true 2>/dev/null; then
    sudo apt-get update
    sudo apt-get install -y "${need_packages[@]}"
  else
    cat >&2 <<EOF
Missing packages: ${need_packages[*]}
sudo requires an interactive password. Run these commands in Ubuntu, then rerun this script:

  sudo apt-get update
  sudo apt-get install -y ${need_packages[*]}
EOF
    exit 3
  fi
fi

run mkdir -p "$LEGACY_STATE_DIR" "$HOME/.claude-science/logs"

if [ ! -x "$PYTHON_BIN" ]; then
  run python3 -m venv "$VENV_DIR"
fi

run "$PYTHON_BIN" -m pip install --upgrade pip
run "$PYTHON_BIN" -m pip install -r "$PROJECT_DIR/requirements.txt"

if [ "$DRY_RUN" = "1" ]; then
  say "+ stage Bridge into $CSA_BRIDGE_ROOT/versions and atomically activate current"
  say "+ verify Claude Science version/hash, reject implicit downgrade, then activate $CSA_CLAUDE_ROOT/current"
else
  csa_stage_bridge_runtime "$PROJECT_DIR" "${CSA_PACKAGE_VERSION:-0.1.7}"
  csa_stage_claude_runtime "$PROJECT_DIR"
fi

if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" = "systemd" ]; then
  if [ "$DRY_RUN" = "1" ]; then
    say "+ install systemd user service from the stable Bridge current pointer"
  else
    run env "PROXY_PORT=$PROXY_PORT" "PYTHON=$PYTHON_BIN" "CSA_STATE_ROOT=$CSA_STATE_ROOT" \
      "CSA_PACKAGE_DIR=$PROJECT_DIR" "CSA_BRIDGE_RUNTIME_ID=$CSA_BRIDGE_RUNTIME_ID" \
      "CSA_BRIDGE_VERSION=$CSA_BRIDGE_VERSION" "CSA_BRIDGE_SOURCE_SHA256=$CSA_BRIDGE_SOURCE_SHA256" \
      bash "$PROJECT_DIR/scripts/install-wsl-bridge-service.sh"
  fi
else
  say "systemd is not running as PID 1; start script will use the fallback process mode."
fi

if [ "$START_SERVICES" = "1" ]; then
  run env "PROXY_PORT=$PROXY_PORT" "PYTHON=$PYTHON_BIN" bash "$PROJECT_DIR/scripts/start-claude-science-wsl.sh"
else
  say "Runtime prepared. Services were not started; pass -StartServices to repair-approved.ps1 to start them."
fi
