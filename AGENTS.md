# Agent Operating Manual

This repository is a Windows launcher plus a single WSL runtime for Claude Science. Claude Science and the FastAPI Bridge run inside the selected WSL distro; the Tauri launcher on Windows owns inspection, lifecycle control, Provider settings, and DPAPI-protected API Key selection.

Read this file first, then follow `docs/agent-runbook.md`.

For the verified Windows/WSL runtime topology, security findings, launcher scope,
and phased product backlog, read `docs/architecture-and-product-plan.zh-CN.md`.

## Prime Directive

Do not break the user's network.

Do not introduce or preserve two writable proxy instances across Windows and WSL.
The supported product design is one WSL Bridge controlled by the Windows launcher.
A detected Windows Bridge is legacy state that must be inspected before migration.

Default to safe mode:

- Do not edit Clash, v2rayN, sing-box, VPN, DNS, TUN, Windows system proxy, or hosts settings.
- Do not reload network daemons.
- Do not install a root CA.
- Do not bind port 443.
- Do not print, commit, summarize, or screenshot API keys, OAuth tokens, private keys, or proxy credentials.

If outbound traffic must use the user's local node, set `outbound_proxy_url` in `config.json` or the dashboard. Do not mutate the network tool itself.

## Goal

Make the client usable with DeepSeek, OpenAI, or another OpenAI-compatible API provider.
If the user needs image understanding, choose a vision-capable backend model and preserve image inputs instead of replacing them with text placeholders.

The supported runtime path is:

1. Run `scripts/status-probe.ps1` or the bootstrap Skill inspectors before mutation.
2. Use `scripts/start-claude-science-wsl.ps1`; do not start a second Windows Bridge.
3. Keep the Bridge on WSL loopback `127.0.0.1:9876` and Claude Science on `8765`.
4. Store launcher Key entries with Windows current-user DPAPI; apply only the active entry to the WSL config with mode `0600`.
5. Configure `model_aliases` and `model_list_mode=aliases` only from user input or a live model list; do not ship a fixed default model.
6. Choose `*_upstream_mode=anthropic` for native Anthropic endpoints; otherwise use `openai`.
7. Preserve caller output budgets. Do not add a global `max_tokens` cap; use explicit per-model caps or the narrow 400/422 compatibility retry.
8. Verify `/health`, `/v1/models`, `/v1/messages`, Bridge `source_path`, and config revision.

## Repository Map

- `proxy.py`: FastAPI proxy, Anthropic Messages API to OpenAI Chat Completions translation.
- `setup-token.py`: creates a local fake Claude Science OAuth token when the client uses `~/.claude-science/encryption.key`.
- `launcher/`: Tauri 2 + React/TypeScript Windows launcher.
- `scripts/status-probe.ps1`: structured Windows/WSL/runtime/storage inspection.
- `scripts/start-claude-science-wsl.ps1` / `.sh`: supported WSL lifecycle path.
- `scripts/self-test.ps1`: Python compile and translation self-tests.
- `scripts/verify-proxy.ps1`: end-to-end proxy verification after provider config.
- `scripts/probe-provider-capabilities.ps1`: explicit, billable Provider text/output/tool/reasoning probe; never run during routine startup.
- `skills/bootstrap-claude-science-wsl/`: read-only-first inspection, approved repair, and rollback flow.
- `docs/v0.1-current-pc-verification.zh-CN.md`: current release-candidate evidence and remaining gates.
- `docs/agent-runbook.md`: operational procedure, including legacy Windows-path notes.
- `docs/troubleshooting.md`: failure modes and fixes.
- `config.example.json`: public, sanitized config template.

## Success Criteria

The task is complete when all of these pass:

```powershell
.\scripts\self-test.ps1
.\scripts\verify-proxy.ps1
Invoke-RestMethod http://127.0.0.1:9876/health
Invoke-RestMethod http://127.0.0.1:9876/v1/models
```

And `http://127.0.0.1:9876/api/recent-requests` shows a successful backend request.

For a vision-capable model, also run:

```powershell
.\scripts\verify-proxy.ps1 -VerifyImage
```

Do not claim image support is working until this passes.

## If Blocked

Use `scripts/status-probe.ps1` or the bootstrap Skill inspectors first. They are read-only and safe. Do not guess at network or WSL state.
