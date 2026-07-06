# Publishing Checklist

Before publishing:

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

If a backend API key is configured locally, optionally run:

```powershell
.\scripts\verify-proxy.ps1
```

Do not publish API keys, OAuth tokens, proxy credentials, generated certificates, or local logs.
