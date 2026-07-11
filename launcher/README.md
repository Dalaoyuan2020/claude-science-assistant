# CSA Windows Launcher

Tauri 2 + Rust + React/TypeScript launcher for inspecting and controlling the CSA WSL runtime.

## Development

```powershell
pnpm install
pnpm build
cargo test --manifest-path src-tauri\Cargo.toml
pnpm tauri build --debug --no-bundle
```

The UI must not run blocking WSL or PowerShell work on the main thread. Runtime state comes from the structured WSL inspector, and service actions must use bounded process timeouts.

Production packages are built from the repository root:

```powershell
.\scripts\package-launcher-portable.ps1 -Profile release
```

Release packaging always recompiles the EXE. `-SkipBuild` is intentionally rejected for release packages.
