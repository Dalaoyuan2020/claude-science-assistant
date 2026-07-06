---
name: bootstrap-claude-science-wsl
description: Inspect, diagnose, install, or repair the Windows 10/11, WSL2, Ubuntu, Claude Science, and local API Bridge environment used by Claude Science Assistant. Use when a user asks to check whether a PC can run Claude Science, diagnose WSL or launcher failures, prepare a new Windows computer, install the domestic-model environment, find duplicate Bridge processes, or produce a redacted support report. Default to read-only inspection; require explicit confirmation before enabling Windows features, installing WSL, rebooting, changing services, or removing files.
---

# Bootstrap Claude Science WSL

Use deterministic scripts to assess the machine before proposing changes. Never infer readiness from one successful port check.

## Workflow

1. Run `scripts/inspect-windows.ps1` in PowerShell without elevation.
2. If a supported WSL distro exists, run `scripts/inspect-wsl.sh` inside that distro.
3. Classify every finding as `pass`, `warning`, or `blocker` using `references/result-schema.md`.
4. Explain the smallest repair plan, including administrator rights, downloads, disk use, and reboot requirements.
5. Stop and obtain explicit user confirmation before any mutating action.
6. After an approved install or repair, rerun both inspectors and the project self-tests.
7. Do not claim the model connection works until an explicitly authorized endpoint test succeeds. Do not make billable model requests by default.

## Inspect Windows

Run from the skill directory or pass the project path:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/inspect-windows.ps1 `
  -ProjectRoot "C:\path\to\claude-science-api-bridge"
```

The script writes one JSON object to stdout. Treat stderr as diagnostic text only. It must remain read-only.

## Inspect WSL

Convert the skill path with `wslpath`, then run:

```powershell
$report = powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/inspect-windows.ps1 `
  -ProjectRoot "C:\path\to\claude-science-api-bridge" | ConvertFrom-Json
$distro = $report.wsl.preferred_distro
$skillWsl = (wsl.exe -d $distro -- wslpath -a $PWD.Path).Trim()
wsl.exe -d $distro -- bash "$skillWsl/scripts/inspect-wsl.sh"
```

Ubuntu-24.04 is recommended, but use the distro returned by the Windows report instead of assuming it exists.

## Safety Boundaries

- Never print API keys, OAuth tokens, proxy credentials, path secrets, full prompts, or encryption keys.
- Report whether a secret exists as a boolean only.
- Do not edit Clash, v2rayN, sing-box, VPN, TUN, DNS, hosts, Windows system proxy, certificate trust, or port 443.
- Do not stop a process by a broad name. Require an exact managed executable path, PID, and command-line match.
- Do not start a Windows Bridge when a WSL Bridge is the selected runtime.
- Do not silently select or trust a prefilled third-party relay.
- Do not install WSL, enable optional Windows features, register startup tasks, or reboot without confirmation.

## Repair Planning

Read `references/support-matrix.md` when selecting a distro, runtime, or unsupported-state response. Read `references/rollback.md` before proposing removal or migration from a legacy Windows Bridge.

Use this sequence for approved changes:

```text
detect -> plan -> confirm -> apply one stage -> verify -> continue or roll back
```

If Windows requests a reboot, record the completed stage and stop. Continue only after the machine returns and inspection confirms the expected feature state.

For a confirmed bootstrap or repair, use `scripts/repair-approved.ps1` as the orchestrator instead of hand-writing WSL setup commands:

```powershell
# Preview only; applies no changes.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/repair-approved.ps1 `
  -ProjectRoot "C:\path\to\claude-science-api-bridge" -PlanOnly

# After explicit user confirmation.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/repair-approved.ps1 `
  -ProjectRoot "C:\path\to\claude-science-api-bridge" -ApproveInstall -StartServices
```

Use `-InstallWslIfMissing` only after confirming the user understands it may enable Windows features, download Ubuntu, require administrator rights, and request a reboot. The WSL-side helper `scripts/bootstrap-wsl-runtime.sh` creates the project-managed WSL virtual environment, installs Python requirements, registers the WSL user service when systemd is available, and optionally starts Claude Science.

For a confirmed rollback or uninstall test, use `scripts/rollback-approved.ps1`. Preview first and preserve user secrets/config by default:

```powershell
# Preview only; applies no changes.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/rollback-approved.ps1 `
  -ProjectRoot "C:\path\to\claude-science-api-bridge" -PlanOnly

# After explicit user confirmation.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/rollback-approved.ps1 `
  -ProjectRoot "C:\path\to\claude-science-api-bridge" -ApproveUninstall
```

Use `-DeleteProductData` only as a separate confirmation. It removes product logs and empty product state directories, but still preserves Provider config, API keys, OAuth tokens, and the original Claude Science binary.

## Output

Return:

1. Overall status: ready, repairable, reboot-required, or unsupported.
2. A compact table of Windows, WSL, Bridge, Claude Science, ports, and duplicate-instance checks.
3. A numbered repair plan with explicit mutation and reboot markers.
4. The exact next command only when it is safe.
5. A redacted diagnostic JSON attachment or path when requested.
