# Publishing Checklist

Before publishing:

```powershell
.\scripts\status-probe.ps1
.\scripts\self-test.ps1
Push-Location launcher\src-tauri
cargo fmt --check
cargo test
Pop-Location
.\scripts\package-launcher-portable.ps1 -Profile release
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

Release packaging must compile the EXE; `-SkipBuild` is not allowed for `-Profile release`. Confirm the package manifest records the intended source commit and `sourceTreeDirty=false`, then upload both:

```text
claude-science-assistant-vX.Y.Z-release-portable.zip
claude-science-assistant-vX.Y.Z-release-portable.zip.sha256
```

After upload, compare the GitHub asset names, sizes, and SHA256 with the local files. Do not push or create a Release until the user approves the reviewed publishing plan.
