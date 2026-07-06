# Troubleshooting for Windows

## Proxy Is Not Running

Check:

```powershell
.\scripts\doctor.ps1
Get-ScheduledTask -TaskName ClaudeScienceByokProxy
Get-Content "$HOME\.claude-science\logs\proxy.log" -Tail 120
```

Restart:

```powershell
.\scripts\start-claude-science.ps1
```

## Backend 400: Invalid Tool Schema

The proxy sanitizes tool schemas before sending them to OpenAI-compatible APIs.
If this still appears, capture only the backend error text from the log. Do not log full prompts or API keys.

## Backend 400: max_tokens Too Large

Some providers reject large `max_tokens` values.

Set a per-model cap:

```json
{
  "model_token_caps": {
    "provider-model-name": 8192
  }
}
```

Then restart the proxy and rerun:

```powershell
.\scripts\verify-proxy.ps1
```

## Requests Do Not Use Local Node

This project does not read `HTTP_PROXY` or `HTTPS_PROXY`, because backend clients use `trust_env=False`.

Set the explicit outbound proxy:

```json
{
  "outbound_proxy_url": "http://127.0.0.1:7890"
}
```

Then restart:

```powershell
.\scripts\start-claude-science.ps1
```

Do not change Clash, v2rayN, sing-box, DNS, TUN, Windows system proxy, hosts, certificates, or port 443 just to make this project use a node.

## Tool Call Markers Appear As Text

Some OpenAI-compatible providers emit native tool-call markers in normal text.
The proxy converts these markers into Anthropic `tool_use` blocks.

If markers still appear:

```powershell
.\scripts\self-test.ps1
.\scripts\start-claude-science.ps1
```

Then check:

```powershell
Invoke-RestMethod http://127.0.0.1:9876/api/recent-requests
```

## Client Shows Connection Issue

Check:

```powershell
Invoke-RestMethod http://127.0.0.1:9876/health
Invoke-RestMethod http://127.0.0.1:9876/api/recent-requests
Get-Content "$HOME\.claude-science\logs\proxy.log" -Tail 120
```

For slow streaming providers, the proxy emits Anthropic-style `ping` events while the upstream stream is idle after `message_start`.

## Requests Return 403 After Enabling Path-Secret

If `proxy_auth_mode=required`, clients must use:

```text
http://127.0.0.1:9876/<secret>
```

Run:

```powershell
.\scripts\start-claude-science.ps1
```

The script reads `config.json`, appends the secret automatically, and masks it in output.

## SSL Certificate Verify Failed When Proxy Calls Backend

If the backend is only reachable through a local proxy or corporate gateway, set:

```json
{
  "outbound_proxy_url": "http://127.0.0.1:7890"
}
```

If that does not apply, confirm the backend base URL is correct and the local Python environment has current certificates.

## Empty Content From Reasoning Models

Some reasoning models put early tokens in `reasoning_content`.
The proxy supports:

- `never`: ignore reasoning content. Default and safest.
- `fallback`: use normal content, or reasoning if content is empty.
- `always`: prepend reasoning content when present.

Recommended:

```json
{
  "reasoning_content_policy": "never"
}
```

## Image Input Fails

First check whether the backend model supports vision input.

Text-only models:

```json
{
  "inline_image_policy": "omit"
}
```

Vision models:

```json
{
  "inline_image_policy": "preserve"
}
```

Then run:

```powershell
.\scripts\verify-proxy.ps1 -VerifyImage
```

## Port 9876 Is Busy

Find the process:

```powershell
Get-NetTCPConnection -LocalPort 9876 -State Listen | Select-Object LocalAddress, LocalPort, OwningProcess
Get-Process -Id <OwningProcess>
```

Either stop it or set a different `PROXY_PORT`, then update `ANTHROPIC_BASE_URL`.

## Verification Fails

Run:

```powershell
.\scripts\doctor.ps1
.\scripts\self-test.ps1
.\scripts\verify-proxy.ps1
```

If `verify-proxy.ps1` says no backend API key is configured, configure the dashboard or write the key to local `config.json`. Do not commit `config.json`.
