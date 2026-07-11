# Claude Science runtime binary

This directory contains the required locked Linux x64 Claude Science runtime used when building the CSA portable release package.

The binary itself is not committed to the source repository because it is a large captured runtime artifact. Official release ZIPs carry it under `vendor/` and are uploaded through GitHub Releases together with their `.sha256` file.

For local release packaging, place the runtime binary at:

```text
vendor/claude-science/linux-x64/claude-science
```

Then update `manifest.json` and `claude-science.sha256` to match that local binary before running:

```powershell
.\scripts\package-launcher-portable.ps1 -Profile release
```

Release packaging verifies the binary against `manifest.json`, records the EXE and runtime hashes, and refuses to reuse a skipped build.
