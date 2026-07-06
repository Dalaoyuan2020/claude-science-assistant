# Agent Operating Manual

This repository is configured for Windows-first local use. It lets Claude Science or any Anthropic-compatible desktop client call a local proxy, while the proxy sends requests to DeepSeek, OpenAI, Kimi, Qwen, or another OpenAI-compatible provider.

Read this file first, then follow `docs/agent-runbook.md`.

For the verified Windows/WSL runtime topology, security findings, launcher scope,
and phased product backlog, read `docs/architecture-and-product-plan.zh-CN.md`.

## Prime Directive

Do not break the user's network.

Do not introduce or preserve two writable proxy instances across Windows and WSL.
The product target is one WSL Bridge controlled by a Windows launcher. Treat the
current Windows + WSL dual-instance state as a migration issue, not a supported design.

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

The safe Windows path is:

1. Run a local HTTP proxy on `127.0.0.1:9876`.
2. Set user-level `ANTHROPIC_BASE_URL=http://127.0.0.1:9876`.
3. Configure API key, backend, model mapping, and optional `outbound_proxy_url` in `config.json` or the dashboard.
4. Configure `model_aliases` and `model_list_mode=aliases` so the client can show third-party model names.
5. Choose `*_upstream_mode=anthropic` for providers with native Anthropic endpoints; otherwise use `openai`.
6. Set `inline_image_policy=preserve` or `auto` only when the selected backend supports image input.
7. Optionally enable `proxy_auth_mode=required` only when the launch path includes the secret.
8. Start or restart the proxy with `scripts/start-claude-science.ps1`.
9. Verify `/v1/models` and `/v1/messages` reach the proxy and the backend succeeds.

## Repository Map

- `proxy.py`: FastAPI proxy, Anthropic Messages API to OpenAI Chat Completions translation.
- `setup-token.py`: creates a local fake Claude Science OAuth token when the client uses `~/.claude-science/encryption.key`.
- `scripts/doctor.ps1`: read-only Windows state inspection.
- `scripts/install-safe.ps1`: Windows safe install, user environment variable, scheduled task.
- `scripts/start-claude-science.ps1`: refreshes `ANTHROPIC_BASE_URL` and starts the proxy task or foreground process.
- `scripts/self-test.ps1`: Python compile and translation self-tests.
- `scripts/verify-proxy.ps1`: end-to-end proxy verification after provider config.
- `scripts/uninstall.ps1`: removes the scheduled task and user environment variable only.
- `docs/agent-runbook.md`: step-by-step Windows procedure for agents.
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

Use `scripts/doctor.ps1` first. It is read-only and safe. Do not guess at network state.
