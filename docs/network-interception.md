# Windows Network Notes

Default mode does not intercept HTTPS traffic and does not edit system networking.

Use this project as:

```text
client -> http://127.0.0.1:9876 -> provider API
```

If provider API traffic should leave through a local node, configure:

```json
{
  "outbound_proxy_url": "http://127.0.0.1:7890"
}
```

Supported proxy URL forms depend on `httpx`, but normal HTTP proxy URLs are expected:

```text
http://127.0.0.1:7890
http://user:password@127.0.0.1:7890
```

## What This Project Should Not Do

- Do not edit Clash, v2rayN, sing-box, VPN, DNS, TUN, Windows system proxy, or hosts.
- Do not install root certificates.
- Do not bind port 443.
- Do not perform transparent HTTPS interception.

## Verification

After setting `outbound_proxy_url`, restart the proxy:

```powershell
.\scripts\start-claude-science.ps1
```

Then test with a real provider key:

```powershell
.\scripts\verify-proxy.ps1
```

If the provider fails, inspect only backend error summaries and redact secrets.
