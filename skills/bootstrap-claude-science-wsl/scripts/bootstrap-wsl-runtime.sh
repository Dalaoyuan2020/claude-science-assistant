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
STATE_DIR="$HOME/.local/share/claude-science-api-bridge"
VENV_DIR="$STATE_DIR/venv"
PYTHON_BIN="$VENV_DIR/bin/python"
MANAGED_CLAUDE_DIR="$STATE_DIR/bin"
MANAGED_CLAUDE_BIN="$MANAGED_CLAUDE_DIR/claude-science"

if [ -z "$PROJECT_DIR" ]; then
  echo "Usage: bootstrap-wsl-runtime.sh /path/to/claude-science-api-bridge" >&2
  exit 2
fi

PROJECT_DIR="$(cd "$PROJECT_DIR" && pwd)"
if [ ! -f "$PROJECT_DIR/proxy.py" ] || [ ! -f "$PROJECT_DIR/requirements.txt" ]; then
  echo "Project root is invalid: $PROJECT_DIR" >&2
  exit 2
fi
BUNDLED_CLAUDE_DIR="$PROJECT_DIR/vendor/claude-science/linux-x64"
BUNDLED_CLAUDE_BIN="$BUNDLED_CLAUDE_DIR/claude-science"
BUNDLED_CLAUDE_SHA="$BUNDLED_CLAUDE_DIR/claude-science.sha256"

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

run mkdir -p "$STATE_DIR" "$HOME/.claude-science/logs"

for skill_name in csa-connect csa-external-agent; do
  skill_source="$PROJECT_DIR/skills/$skill_name"
  if [ -f "$skill_source/SKILL.md" ]; then
    skill_target="$HOME/.claude-science/skills/$skill_name"
    run mkdir -p "$skill_target"
    run cp -R "$skill_source/." "$skill_target/"
    run chmod -R u+rwX,go-rwx "$skill_target"
    if [ -d "$skill_target/scripts" ]; then
      run chmod 700 "$skill_target/scripts"/*.sh
    fi
  fi
done

if [ -f "$BUNDLED_CLAUDE_BIN" ]; then
  say "Bundled Claude Science Linux binary found; it will be installed as the locked product-managed runtime binary."
  run mkdir -p "$MANAGED_CLAUDE_DIR"
  run cp -f "$BUNDLED_CLAUDE_BIN" "$MANAGED_CLAUDE_BIN"
  run chmod 755 "$MANAGED_CLAUDE_BIN"
  if [ "$DRY_RUN" = "1" ]; then
    say "+ verify sha256 using $BUNDLED_CLAUDE_SHA"
  elif [ -f "$BUNDLED_CLAUDE_SHA" ]; then
    (cd "$MANAGED_CLAUDE_DIR" && sha256sum -c "$BUNDLED_CLAUDE_SHA")
  fi
elif [ -x "$HOME/.local/bin/claude-science" ]; then
  say "Bundled Claude Science binary was not found; falling back to existing ~/.local/bin/claude-science."
else
  cat >&2 <<EOF
Claude Science Linux binary is not installed and no bundled binary was found.
Use the full portable package that contains vendor/claude-science/linux-x64/claude-science,
or install the supported Claude Science binary manually before starting services.
EOF
  exit 4
fi

if [ ! -x "$PYTHON_BIN" ]; then
  run python3 -m venv "$VENV_DIR"
fi

run "$PYTHON_BIN" -m pip install --upgrade pip
run "$PYTHON_BIN" -m pip install -r "$PROJECT_DIR/requirements.txt"

if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" = "systemd" ]; then
  run env "PROXY_PORT=$PROXY_PORT" "PYTHON=$PYTHON_BIN" bash "$PROJECT_DIR/scripts/install-wsl-bridge-service.sh"
else
  say "systemd is not running as PID 1; start script will use the fallback process mode."
fi

if [ "$START_SERVICES" = "1" ]; then
  run env "PROXY_PORT=$PROXY_PORT" "PYTHON=$PYTHON_BIN" bash "$PROJECT_DIR/scripts/start-claude-science-wsl.sh"
else
  say "Runtime prepared. Services were not started; pass -StartServices to repair-approved.ps1 to start them."
fi
