# Contributing

## Development Setup

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements-dev.txt
.\scripts\self-test.ps1
.\scripts\doctor.ps1
```

## Safety Rules

- Keep safe mode as the default path.
- Do not add scripts that silently modify Clash, v2rayN, sing-box, VPN, TUN, DNS, Windows system proxy, hosts, certificates, or port 443.
- Use `outbound_proxy_url` for backend egress through a local node.
- Do not log request bodies by default.
- Do not commit generated certificates, API keys, local OAuth tokens, proxy credentials, or logs.

## Pull Request Checklist

- `.\scripts\self-test.ps1` passes.
- If a backend API key is configured on the machine, `.\scripts\verify-proxy.ps1` passes.
- Optional: `.\.venv\Scripts\python.exe -m pytest -q` passes.
- README and `AGENTS.md` still match behavior.
- No secrets are staged.
