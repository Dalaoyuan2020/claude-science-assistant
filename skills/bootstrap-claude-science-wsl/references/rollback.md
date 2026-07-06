# Rollback boundaries

Rollback only artifacts owned by Claude Science Assistant:

- Its WSL user services.
- Its patched Claude Science copy under `~/.local/share/claude-science-api-bridge/`.
- Its project-managed virtual environment.
- Its launcher files and explicitly created startup entry.

Preserve by default:

- The original Claude Science binary.
- The user's WSL distribution and home directory.
- Provider configuration and secrets.
- Unrelated Python environments, scheduled tasks, network tools, and system settings.

Before stopping a legacy Windows Bridge, verify the PID, executable path, and command line refer to this project. Never stop every `python.exe` process.

Before deleting any data, show exact resolved paths and obtain explicit confirmation. Uninstall and “delete all user data” must remain separate actions.
