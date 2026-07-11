# Agent Runbook for Windows

This is the agent-facing operations guide. The supported v0.1.3 product path is a Windows launcher controlling one WSL Bridge and one WSL Claude Science runtime.

Before installing or starting anything, read `v0.1-current-pc-verification.zh-CN.md`
and check both Windows and WSL for an existing Bridge. Never start the legacy
Windows Bridge alongside the WSL Bridge; that dual-instance state produces split
configuration and request logs.

Default to safe mode. Do not modify Clash, v2rayN, sing-box, VPN, TUN, DNS, Windows system proxy, hosts, certificate trust, or port 443.

## Phase 0: Safety Check

Run only read-only checks first:

```powershell
.\scripts\doctor.ps1
```

Inspect:

- Windows version
- Python path and version
- whether `ANTHROPIC_BASE_URL` is already set
- whether ports `9876`, `9877`, `443`, and `8765` are in use
- whether `config.json` exists
- whether the proxy is already healthy

Do not change any network proxy tool.

## Supported v0.1.3 Workflow

Preview first:

```powershell
.\scripts\acceptance-v0.1.ps1
.\scripts\status-probe.ps1
```

After explicit user approval, install or repair the WSL runtime through the bundled Skill or acceptance helper:

```powershell
.\scripts\acceptance-v0.1.ps1 -ApproveInstall -StartServices -RunSelfTest
```

Use `-InstallWslIfMissing` only after separate confirmation of administrator rights, Windows feature changes, downloads, and a possible reboot. Normal lifecycle startup uses:

```powershell
.\scripts\start-claude-science-wsl.ps1
```

Provider changes should go through the launcher transaction so Windows settings and WSL Bridge configuration either commit together or roll back. Verify with:

```powershell
.\scripts\status-probe.ps1
.\scripts\self-test.ps1
.\scripts\verify-proxy.ps1
```

## Legacy Windows Bridge Reference

The remaining `install-safe.ps1`, scheduled-task, and `start-claude-science.ps1` sections describe the old Windows Bridge route. Keep them only for migration and diagnostics; do not use them as the default launcher workflow.

## Phase 1: Install Safe Mode

Safe mode does not modify hosts, certificates, Clash, DNS, TUN, VPN, Windows system proxy, or port 443.

```powershell
.\scripts\install-safe.ps1
```

This should:

1. Install Python dependencies into project-local `.venv`.
2. Create `config.json` from `config.example.json` if missing.
3. Generate a fake OAuth token only if `~\.claude-science\encryption.key` exists.
4. Set user-level `ANTHROPIC_BASE_URL`.
5. Install and start the current-user scheduled task `ClaudeScienceByokProxy`.

The script does not patch desktop app binaries on Windows.

## Phase 2: Configure Provider

Prefer the dashboard:

```powershell
Start-Process http://127.0.0.1:9876/dashboard
```

For unattended setup, write `config.json` directly. Never echo secrets into chat logs.
`scripts/install-safe.ps1` also accepts provider settings from environment variables and persists them into ignored `config.json`.

First decide the upstream protocol:

- Use `*_upstream_mode=anthropic` when the provider has a native Anthropic Messages endpoint.
- Use `*_upstream_mode=openai` when the provider only exposes an OpenAI-compatible endpoint.

Minimum DeepSeek config:

```json
{
  "deepseek_api_key": "REDACTED",
  "deepseek_base_url": "https://api.deepseek.com/anthropic",
  "default_backend": "deepseek",
  "force_model": "<model returned by the account or explicitly confirmed by the user>"
}
```

For a MiniMax China account, use `https://api.minimaxi.com/anthropic` with
`custom_upstream_mode=anthropic`. The launcher preset leaves the model empty;
`MiniMax-M3` is an official model ID, but it must still be tested with the user's account.

Generic OpenAI-compatible provider:

```json
{
  "custom_api_key": "REDACTED",
  "custom_base_url": "https://provider.example.com",
  "custom_upstream_mode": "openai",
  "default_backend": "custom",
  "force_model": "provider-model-name",
  "model_aliases": [
    {
      "id": "byok-model-0001",
      "display_name": "Provider Model",
      "backend": "custom",
      "model": "provider-model-name"
    }
  ],
  "model_list_mode": "aliases",
  "inline_image_policy": "auto"
}
```

For SiliconFlow Kimi:

```json
{
  "custom_api_key": "REDACTED",
  "custom_base_url": "https://api.siliconflow.cn",
  "custom_upstream_mode": "openai",
  "default_backend": "custom",
  "force_model": "Pro/moonshotai/Kimi-K2.6",
  "model_aliases": [
    {
      "id": "byok-model-0001",
      "display_name": "Kimi K2.6 Pro++",
      "backend": "custom",
      "model": "Pro/moonshotai/Kimi-K2.6"
    }
  ],
  "model_list_mode": "aliases",
  "inline_image_policy": "preserve",
  "reasoning_content_policy": "never"
}
```

## Phase 2.1: Optional Outbound Proxy

If the user wants backend requests to go through a local node, configure the proxy URL:

```json
{
  "outbound_proxy_url": "http://127.0.0.1:7890"
}
```

For authenticated local proxies:

```json
{
  "outbound_proxy_url": "http://user:password@127.0.0.1:7890"
}
```

Do not set Windows system proxy for this project. The Python HTTP client intentionally uses `trust_env=False`; use `outbound_proxy_url`.

## Phase 2.2: Image Policy

Use `inline_image_policy=preserve` only when the selected model supports image input. Use `omit` for text-only models.
Keep `reasoning_content_policy=never` unless the user explicitly asks to debug provider reasoning payloads.

If the provider rejects large `max_tokens`, set caps:

```json
{
  "model_token_caps": {
    "provider-model-name": 8192
  },
  "default_max_tokens_cap": 0
}
```

## Phase 2.3: Optional Path-Secret

To reduce local misuse of the user's third-party key:

```json
{
  "proxy_auth_token": "REDACTED_RANDOM_SECRET",
  "proxy_auth_mode": "required"
}
```

Then rerun:

```powershell
.\scripts\start-claude-science.ps1
```

The script sets `ANTHROPIC_BASE_URL` to `http://127.0.0.1:9876/<secret>` and masks the secret in output.

## Phase 3: Verify Proxy

Run:

```powershell
.\scripts\self-test.ps1
.\scripts\verify-proxy.ps1
Invoke-RestMethod http://127.0.0.1:9876/health
Invoke-RestMethod http://127.0.0.1:9876/v1/models
```

Expected:

- `/health` returns `"status":"ok"`.
- `/v1/models` returns model objects.
- `/v1/messages` returns an Anthropic-style message object during `verify-proxy.ps1`.
- `/api/recent-requests` shows backend `success`.

If the selected model is vision-capable:

```powershell
.\scripts\verify-proxy.ps1 -VerifyImage
```

Do not report image support as working until this test passes.

## Phase 4: Start Client

The proxy can be started or refreshed with:

```powershell
.\scripts\start-claude-science.ps1
```

Then launch the desktop client normally. It should read the user-level `ANTHROPIC_BASE_URL` for future processes. If the client was already running, restart it.

Verify recent requests:

```powershell
Invoke-RestMethod http://127.0.0.1:9876/api/recent-requests
```

Expected requests include `GET /v1/models` and `POST /v1/messages`.

## Phase 5: Cleanup

To remove safe-mode installation:

```powershell
.\scripts\uninstall.ps1
```

This removes the scheduled task and user-level environment variable. It does not delete API keys unless the user asks.

## Phase 6: Before Publishing

Run:

```powershell
.\scripts\self-test.ps1
git status --ignored
```

Confirm ignored local files are not staged:

- `config.json`
- `.env`
- `certs/`
- logs
- Python caches
- `.venv/`
