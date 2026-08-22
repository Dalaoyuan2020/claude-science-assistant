# Troubleshooting for Windows

## Proxy Is Not Running

Check the supported WSL runtime:

```powershell
.\scripts\status-probe.ps1
```

Restart:

```powershell
.\scripts\start-claude-science-wsl.ps1
```

## Backend 400: Invalid Tool Schema

The proxy sanitizes tool schemas before sending them to OpenAI-compatible APIs.
If this still appears, capture only the backend error text from the log. Do not log full prompts or API keys.

## Backend 400: max_tokens Too Large

Some providers reject large `max_tokens` values.

CSA does not apply one global output length to normal Claude Science requests. The Bridge preserves the caller's `max_tokens`; if the caller omits it for an OpenAI-compatible upstream, the field is omitted so the upstream can use its model default. `default_max_tokens_cap=0` means “do not clamp”.

Only add a cap after the provider has returned a documented limit error. Do not use a very small blanket value: reasoning tokens, visible text, and tool-call arguments can all consume the same output budget, so an undersized cap can produce HTTP 200 with no visible answer or stop an agent before it calls a tool.

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

The CSA Bridge does not use ambient `HTTP_PROXY` / `HTTPS_PROXY`; its backend client uses
`trust_env=False`. Claude Science and its sandbox network process are separate: the daemon can
inherit proxy variables when it starts. The managed launcher now validates every inherited
HTTP/HTTPS/ALL proxy endpoint and refuses to reuse a daemon whose proxy has gone away.

Set the explicit outbound proxy:

```json
{
  "outbound_proxy_url": "http://127.0.0.1:7890"
}
```

Then restart:

```powershell
.\scripts\start-claude-science-wsl.ps1
```

Do not change Clash, v2rayN, sing-box, DNS, TUN, Windows system proxy, hosts, certificates, or port 443 just to make this project use a node.

## OpenAlex / arXiv or Other Sandbox Requests Return 502

Do not diagnose this from ports `8765`, `8766`, or the sandbox forwarder alone. Those listeners can
remain open after the daemon's inherited upstream proxy has stopped. Run:

```powershell
.\scripts\status-probe.ps1 -DeepNetworkProbe
```

The report separates four facts: managed process ownership, inherited proxy endpoint reachability,
sandbox HTTP forwarder ownership, and an anonymous/non-billable arXiv request through the real
sandbox route. Proxy URLs are reduced to scheme/host/port and credentials are never included.

If `proxy_state` is `unreachable` or `conflict`, use the launcher's **修复并重启** action after the
current experiment finishes. A Bridge-only restart cannot refresh proxy variables already inherited
by Claude Science. The controlled restart does not modify the Windows/WSL system proxy, VPN, DNS,
hosts, certificates, or port 443.

## Tool Call Markers Appear As Text

Some OpenAI-compatible providers emit native tool-call markers in normal text.
The proxy converts these markers into Anthropic `tool_use` blocks.

If markers still appear:

```powershell
.\scripts\self-test.ps1
.\scripts\start-claude-science-wsl.ps1
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

Reopen the dashboard from the launcher so it uses the required path secret. If the WSL runtime is stopped, run:

```powershell
.\scripts\start-claude-science-wsl.ps1
```

Do not manually print or paste the path secret.

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

The launcher connectivity test uses a bounded adaptive budget: it starts at 256 tokens and retries the same model with 1024 only when the first response is length-limited or contains reasoning without visible text. This test budget is separate from normal Claude Science conversations and is never written into the Bridge runtime configuration.
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

`reasoning_content_policy` controls what the Bridge displays; it does not control how much the model thinks. Keep `never` for normal use so private/internal reasoning is not surfaced. If a request ends with `stop_reason=max_tokens`, raise the caller's output budget or lower the model's effort; changing the display policy is not a real fix for truncation.

## A Model Rejects Output, Thinking, or Parallel Tool Parameters

OpenAI-compatible APIs share an endpoint shape, but not one universal parameter set. OpenAI
o-series models use `max_completion_tokens`; GLM/Kimi/DeepSeek-style models, Qwen, MiniMax,
OpenRouter, and SiliconFlow expose different reasoning controls.

CSA translates an explicit Claude Science reasoning request by platform first and model family
second. It does not turn reasoning on when the caller did not ask for it. If an upstream returns a
specific HTTP 400/422 parameter error, the bridge may make one compatibility retry. It does not retry
authentication, quota, model-not-found, network, or 5xx errors.

For a saved Provider, run `scripts/probe-provider-capabilities.ps1` to check the model list, visible
text, a 32768-token parameter probe, native function calling, parallel tool parameter acceptance,
and the applicable reasoning control. This sends real, short API requests and can consume a small
amount of quota; it never prints the saved key or answer body.

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
