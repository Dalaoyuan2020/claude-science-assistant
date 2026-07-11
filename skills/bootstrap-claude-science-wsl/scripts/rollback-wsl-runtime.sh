#!/usr/bin/env bash
set -euo pipefail

# Roll back only Claude Science Assistant-owned WSL runtime artifacts.
# Defaults to dry-run. Set DRY_RUN=0 after explicit user confirmation.

DRY_RUN="${DRY_RUN:-1}"
DELETE_PRODUCT_DATA="${DELETE_PRODUCT_DATA:-0}"
SERVICE_NAME="claude-science-bridge.service"
UNIT_PATH="$HOME/.config/systemd/user/$SERVICE_NAME"
STATE_DIR="$HOME/.local/share/claude-science-api-bridge"
PATCHED_DIR="$STATE_DIR/patched"
VENV_DIR="$STATE_DIR/venv"
LOG_DIR="$HOME/.claude-science/logs"

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

assert_under_home() {
  local path="$1"
  case "$path" in
    "$HOME"/*) return 0 ;;
    *) echo "Refusing to touch path outside HOME: $path" >&2; exit 3 ;;
  esac
}

say "Rollback mode: $(if [ "$DRY_RUN" = "1" ]; then echo dry-run; else echo apply; fi)"
say "Preserving by default: ~/.claude-science/proxy/config.json, API keys, OAuth tokens, original ~/.local/bin/claude-science."

if [ "$(ps -p 1 -o comm= 2>/dev/null | tr -d ' ')" = "systemd" ]; then
  if systemctl --user list-unit-files "$SERVICE_NAME" >/dev/null 2>&1 || [ -f "$UNIT_PATH" ]; then
    run systemctl --user stop "$SERVICE_NAME"
    run systemctl --user disable "$SERVICE_NAME"
  fi
  if [ -f "$UNIT_PATH" ]; then
    assert_under_home "$UNIT_PATH"
    run rm -f "$UNIT_PATH"
    run systemctl --user daemon-reload
  fi
else
  say "systemd is not PID 1; no user service disable step is available."
fi

# Stop only the process that owns CSA's Bridge port and actually runs proxy.py.
for pid in $(ss -ltnp "sport = :9876" 2>/dev/null | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u || true); do
  if [ -r "/proc/$pid/cmdline" ]; then
    cmd="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
    case "$cmd" in
      *"/proxy.py"*) run kill "$pid" ;;
    esac
  fi
done

for pid in $(pgrep -f 'claude-science-api-bridge/patched/claude-science serve' 2>/dev/null || true); do
  if [ -r "/proc/$pid/cmdline" ]; then
    cmd="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true)"
    case "$cmd" in
      *"claude-science-api-bridge/patched/claude-science serve"*) run kill "$pid" ;;
    esac
  fi
done

for path in "$PATCHED_DIR" "$VENV_DIR"; do
  if [ -e "$path" ]; then
    assert_under_home "$path"
    run rm -rf "$path"
  fi
done

if [ "$DELETE_PRODUCT_DATA" = "1" ]; then
  say "DELETE_PRODUCT_DATA=1: removing product logs and empty state directory only; secrets/config remain preserved."
  if [ -d "$LOG_DIR" ]; then
    assert_under_home "$LOG_DIR"
    run rm -rf "$LOG_DIR"
  fi
  if [ -d "$STATE_DIR" ]; then
    assert_under_home "$STATE_DIR"
    run rmdir "$STATE_DIR" 2>/dev/null || true
  fi
fi

say "Rollback plan/apply finished. Re-run inspectors before claiming cleanup."
