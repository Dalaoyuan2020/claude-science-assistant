#!/usr/bin/env bash
set -euo pipefail

cat <<'TEXT'
Transparent network interception is not part of the Windows port.

Use safe mode:
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/install-safe.ps1

If backend traffic should go through a local node, configure config.json:
  "outbound_proxy_url": "http://127.0.0.1:7890"

This script intentionally does not edit hosts, certificates, system proxy,
DNS, VPN, TUN, or port 443.
TEXT
