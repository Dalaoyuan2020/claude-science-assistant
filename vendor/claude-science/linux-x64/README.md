# Claude Science runtime binary

This directory is reserved for the optional Linux x64 Claude Science runtime binary used when building the CSA portable release package.

The binary itself is not committed to the source repository because it is a large captured runtime artifact. Official release ZIPs should be uploaded through GitHub Releases, together with their `.sha256` file.

For local release packaging, place the runtime binary at:

```text
vendor/claude-science/linux-x64/claude-science
```

Then update `manifest.json` and `claude-science.sha256` to match that local binary before running:

```powershell
.\scripts\package-launcher-portable.ps1 -Profile release -SkipBuild
```
