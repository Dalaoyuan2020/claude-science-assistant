# Support matrix

| Component | v0.1 support |
|---|---|
| Windows | Windows 10 22H2 x64 or Windows 11 x64 |
| WSL | WSL2 only |
| Linux distro | Ubuntu 24.04 preferred; recent Ubuntu fallback |
| Init | systemd enabled |
| Launcher | Tauri 2 desktop build |
| Bridge | One WSL user instance only |
| Python | Project-managed virtual environment |

Do not silently convert WSL1 distributions. Explain that conversion stops the distro and request confirmation.

Do not select Docker Desktop distributions. Do not install into `docker-desktop` or `docker-desktop-data`.

Require at least 8 GB free on the Windows system drive for installation planning. Warn below 15 GB.
