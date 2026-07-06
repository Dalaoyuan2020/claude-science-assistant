Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"

$TaskName = "ClaudeScienceByokProxy"
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name $TaskName -ErrorAction SilentlyContinue
[Environment]::SetEnvironmentVariable("ANTHROPIC_BASE_URL", $null, "User")

Write-Host "Removed $TaskName scheduled task, Run fallback, and user ANTHROPIC_BASE_URL."
Write-Host "Left config.json, API keys, OAuth token files, and logs in place."
