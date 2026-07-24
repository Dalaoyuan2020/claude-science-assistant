[CmdletBinding()]
param(
  [string]$ProjectRoot = "",
  [string]$OutputDir = "",
  [string]$TargetSoftwareName = "Claude Code / Claude CLI / CSA local runtime",
  [string]$TargetRuntime = "Windows + WSL + local CLI",
  [string]$TestMachine = "",
  [string[]]$TargetRoots = @(),
  [string[]]$CliTools = @("claude", "codex", "python", "node", "git", "docker", "wsl", "uv", "conda"),
  [ValidateSet("none", "version", "agent")]
  [string]$ExternalCliProbeMode = "version",
  [int]$MaxDepth = 4,
  [int]$MaxFilesPerRoot = 300,
  [int]$MaxSampleBytes = 4096,
  [switch]$IncludeWindowsPorts,
  [switch]$JsonToStdout
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $ProjectRoot) {
  $ProjectRoot = (Resolve-Path -LiteralPath (Join-Path $ScriptDir "..")).Path
} else {
  $ProjectRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
}
if (-not $TestMachine) {
  $TestMachine = if ($env:COMPUTERNAME) { $env:COMPUTERNAME } else { "unknown" }
}

if (-not $OutputDir) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $OutputDir = Join-Path $ProjectRoot "reports\csa-feasibility\$stamp"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$SensitiveNamePattern = '(?i)(^\.env$|\.env\.|secret|token|cookie|credential|password|passwd|private|id_rsa|id_dsa|id_ecdsa|id_ed25519|\.pem$|\.pfx$|\.p12$|\.key$|oauth|apikey|api_key|authorization)'
$SecretValuePattern = '(?i)(sk-[A-Za-z0-9_\-]{16,}|xox[baprs]-[A-Za-z0-9\-]{12,}|gh[pousr]_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_\-]{20,}|Bearer\s+[A-Za-z0-9._\-]+)'

function Redact-Text {
  param([AllowNull()][string]$Text, [int]$MaxChars = 1200)
  if ($null -eq $Text) { return "" }
  $clean = $Text -replace $SecretValuePattern, "[REDACTED_SECRET]"
  $clean = $clean -replace '(?i)(api[_-]?key|token|secret|authorization|cookie|password)\s*[:=]\s*["'']?[^"'',\s\}]+', '$1=[REDACTED]'
  $clean = $clean -replace '[\x00-\x08\x0B\x0C\x0E-\x1F]', ''
  if ($clean.Length -gt $MaxChars) {
    return $clean.Substring(0, $MaxChars) + "...[truncated]"
  }
  return $clean
}

function Join-ProcessArguments {
  param([string[]]$Arguments)
  $escaped = @()
  foreach ($arg in $Arguments) {
    if ($null -eq $arg) {
      $escaped += '""'
      continue
    }
    $text = [string]$arg
    if ($text -notmatch '[\s"]') {
      $escaped += $text
      continue
    }
    $escapedText = $text -replace '(\\*)"', '$1$1\"'
    $escapedText = $escapedText -replace '(\\+)$', '$1$1'
    $escaped += '"' + $escapedText + '"'
  }
  return ($escaped -join " ")
}

function ConvertTo-RelativePath {
  param([string]$Root, [string]$Path)
  try {
    $rootFull = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $pathFull = [IO.Path]::GetFullPath($Path)
    if ($pathFull.StartsWith($rootFull, [StringComparison]::OrdinalIgnoreCase)) {
      return $pathFull.Substring($rootFull.Length)
    }
  } catch {
  }
  return Split-Path -Leaf $Path
}

function Get-RelativeDepth {
  param([string]$RelativePath)
  if (-not $RelativePath) { return 0 }
  return @($RelativePath -split '[\\/]' | Where-Object { $_ }).Count
}

function Invoke-ProcessCapture {
  param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [string[]]$Arguments = @(),
    [int]$TimeoutSeconds = 10,
    [string]$WorkingDirectory = $ProjectRoot
  )

  $psi = [System.Diagnostics.ProcessStartInfo]::new()
  $psi.FileName = $FilePath
  $psi.Arguments = Join-ProcessArguments -Arguments $Arguments
  $psi.WorkingDirectory = $WorkingDirectory
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true

  $proc = [System.Diagnostics.Process]::new()
  $proc.StartInfo = $psi
  $started = $false
  $startedAt = Get-Date
  try {
    $started = $proc.Start()
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    $exited = $proc.WaitForExit($TimeoutSeconds * 1000)
    if (-not $exited) {
      try { $proc.Kill($true) } catch { try { $proc.Kill() } catch { } }
      $elapsedMs = [int]((Get-Date) - $startedAt).TotalMilliseconds
      return [ordered]@{
        ok = $false
        timed_out = $true
        exit_code = $null
        duration_ms = $elapsedMs
        stdout = ""
        stderr = "Timed out after $TimeoutSeconds seconds."
      }
    }
    $elapsedMs = [int]((Get-Date) - $startedAt).TotalMilliseconds
    return [ordered]@{
      ok = ($proc.ExitCode -eq 0)
      timed_out = $false
      exit_code = $proc.ExitCode
      duration_ms = $elapsedMs
      stdout = Redact-Text $stdoutTask.Result
      stderr = Redact-Text $stderrTask.Result
    }
  } catch {
    $elapsedMs = [int]((Get-Date) - $startedAt).TotalMilliseconds
    return [ordered]@{
      ok = $false
      timed_out = $false
      exit_code = $null
      duration_ms = $elapsedMs
      stdout = ""
      stderr = Redact-Text $_.Exception.Message
    }
  } finally {
    if ($started) { $proc.Dispose() }
  }
}

function Invoke-ToolCapture {
  param(
    [Parameter(Mandatory = $true)]$CommandInfo,
    [Parameter(Mandatory = $true)][string]$Tool,
    [string[]]$Arguments = @(),
    [int]$TimeoutSeconds = 10,
    [string]$WorkingDirectory = $ProjectRoot
  )

  $source = [string]$CommandInfo.Source
  $ext = ([IO.Path]::GetExtension($source)).ToLowerInvariant()
  if ($ext -eq ".ps1") {
    return Invoke-ProcessCapture -FilePath "powershell.exe" -Arguments (@("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $source) + $Arguments) -TimeoutSeconds $TimeoutSeconds -WorkingDirectory $WorkingDirectory
  }
  if ($ext -eq ".cmd" -or $ext -eq ".bat") {
    $line = (Join-ProcessArguments -Arguments (@($source) + $Arguments))
    return Invoke-ProcessCapture -FilePath "cmd.exe" -Arguments @("/d", "/c", $line) -TimeoutSeconds $TimeoutSeconds -WorkingDirectory $WorkingDirectory
  }

  $direct = Invoke-ProcessCapture -FilePath $source -Arguments $Arguments -TimeoutSeconds $TimeoutSeconds -WorkingDirectory $WorkingDirectory
  if ($direct.ok -or $direct.timed_out) {
    return $direct
  }

  if (($direct.stderr -match '(?i)access is denied|not a valid application') -and $Tool -match '^[A-Za-z0-9_.-]+$') {
    $line = (Join-ProcessArguments -Arguments (@($Tool) + $Arguments))
    $fallback = Invoke-ProcessCapture -FilePath "cmd.exe" -Arguments @("/d", "/c", $line) -TimeoutSeconds $TimeoutSeconds -WorkingDirectory $WorkingDirectory
    if ($fallback.ok) {
      $fallback.stderr = (Redact-Text ($fallback.stderr + "`nRetried through shell alias because direct launch failed: " + $direct.stderr) 1000)
      return $fallback
    }
  }

  return $direct
}

function Test-SensitiveFileName {
  param([string]$Path)
  $leaf = Split-Path -Leaf $Path
  return [bool]($leaf -match $SensitiveNamePattern)
}

function Get-JsonShape {
  param([string]$Path)
  try {
    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($item.Length -gt 1048576) {
      return [ordered]@{ parsed = $false; reason = "file_too_large_for_shape_probe" }
    }
    $text = [IO.File]::ReadAllText($Path)
    $obj = $text | ConvertFrom-Json -ErrorAction Stop
    $keys = @()
    if ($obj -is [System.Array]) {
      if ($obj.Count -gt 0 -and $obj[0].PSObject) {
        $keys = @($obj[0].PSObject.Properties.Name | Sort-Object -Unique)
      }
      return [ordered]@{ parsed = $true; kind = "json_array"; top_level_keys = $keys; item_count_if_array = $obj.Count }
    }
    if ($obj.PSObject) {
      $keys = @($obj.PSObject.Properties.Name | Sort-Object -Unique)
    }
    return [ordered]@{ parsed = $true; kind = "json_object"; top_level_keys = $keys }
  } catch {
    return [ordered]@{ parsed = $false; reason = "json_parse_failed" }
  }
}

function Get-JsonlShape {
  param([string]$Path, [int]$MaxLines = 80)
  $keys = New-Object System.Collections.Generic.HashSet[string]
  $typeCounts = [ordered]@{}
  $roleCounts = [ordered]@{}
  $parsed = 0
  $linesSeen = 0
  $hasErrorField = $false
  $hasResultLikeField = $false

  try {
    Get-Content -LiteralPath $Path -TotalCount $MaxLines -Encoding UTF8 -ErrorAction Stop | ForEach-Object {
      $line = "$_".Trim()
      if (-not $line) { return }
      $script:__dummy = $null
      $linesSeen += 1
      try {
        $obj = $line | ConvertFrom-Json -ErrorAction Stop
        $parsed += 1
        foreach ($prop in $obj.PSObject.Properties) {
          [void]$keys.Add($prop.Name)
        }
        if ($obj.PSObject.Properties.Name -contains "type") {
          $value = [string]$obj.type
          if ($value -and $value.Length -lt 80) {
            if (-not $typeCounts.Contains($value)) { $typeCounts[$value] = 0 }
            $typeCounts[$value] += 1
          }
        }
        if ($obj.PSObject.Properties.Name -contains "role") {
          $value = [string]$obj.role
          if ($value -and $value.Length -lt 80) {
            if (-not $roleCounts.Contains($value)) { $roleCounts[$value] = 0 }
            $roleCounts[$value] += 1
          }
        }
        foreach ($name in $obj.PSObject.Properties.Name) {
          if ($name -match '(?i)error|exception|failure') { $hasErrorField = $true }
          if ($name -match '(?i)result|summary|status|stop|finish') { $hasResultLikeField = $true }
        }
      } catch {
      }
    }
  } catch {
    return [ordered]@{ parsed = $false; reason = "jsonl_read_failed" }
  }

  return [ordered]@{
    parsed = ($parsed -gt 0)
    lines_sampled = $linesSeen
    objects_sampled = $parsed
    top_level_keys = @($keys | Sort-Object)
    type_counts = $typeCounts
    role_counts = $roleCounts
    has_error_like_field = $hasErrorField
    has_result_like_field = $hasResultLikeField
  }
}

function Get-FileFormatSummary {
  param([string]$Path)
  $ext = ([IO.Path]::GetExtension($Path)).ToLowerInvariant()
  if (Test-SensitiveFileName $Path) {
    return [ordered]@{ source_kind = "machine_test"; format = "sensitive_name_skipped"; content_sampled = $false }
  }

  switch ($ext) {
    ".json" { return [ordered]@{ source_kind = "machine_test"; format = "json"; content_sampled = $true; shape = Get-JsonShape $Path } }
    ".jsonl" { return [ordered]@{ source_kind = "machine_test"; format = "jsonl"; content_sampled = $true; shape = Get-JsonlShape $Path } }
    ".sqlite" { return [ordered]@{ source_kind = "machine_test"; format = "sqlite"; content_sampled = $false } }
    ".db" { return [ordered]@{ source_kind = "machine_test"; format = "sqlite_or_database"; content_sampled = $false } }
    ".log" { return [ordered]@{ source_kind = "machine_test"; format = "log_text"; content_sampled = $false } }
    ".toml" { return [ordered]@{ source_kind = "machine_test"; format = "toml_text"; content_sampled = $false } }
    ".yaml" { return [ordered]@{ source_kind = "machine_test"; format = "yaml_text"; content_sampled = $false } }
    ".yml" { return [ordered]@{ source_kind = "machine_test"; format = "yaml_text"; content_sampled = $false } }
    default {
      try {
        $limit = [Math]::Max(16, $MaxSampleBytes)
        $buffer = New-Object byte[] $limit
        $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
        try {
          $read = $stream.Read($buffer, 0, $buffer.Length)
        } finally {
          $stream.Dispose()
        }
        if ($read -le 0) {
          return [ordered]@{ source_kind = "machine_test"; format = "empty"; content_sampled = $false }
        }
        $sample = $buffer[0..($read - 1)]
        $nulCount = @($sample | Where-Object { $_ -eq 0 }).Count
        if ($nulCount -gt 0) {
          return [ordered]@{ source_kind = "machine_test"; format = "binary_or_encrypted_like"; content_sampled = $false }
        }
      } catch {
      }
      return [ordered]@{ source_kind = "machine_test"; format = "unknown_or_text"; content_sampled = $false }
    }
  }
}

function Get-BoundedFiles {
  param(
    [string]$Root,
    [int]$DepthLimit,
    [int]$FileLimit,
    [string]$Filter = "*"
  )
  $results = New-Object System.Collections.Generic.List[object]
  $queue = New-Object System.Collections.Queue
  $queue.Enqueue([ordered]@{ path = $Root; depth = 0 })

  while ($queue.Count -gt 0 -and $results.Count -lt $FileLimit) {
    $current = $queue.Dequeue()
    $path = [string]$current.path
    $depth = [int]$current.depth

    try {
      $files = @(
        Get-ChildItem -LiteralPath $path -File -Filter $Filter -ErrorAction SilentlyContinue |
          Sort-Object LastWriteTimeUtc -Descending
      )
      foreach ($file in $files) {
        if ($results.Count -ge $FileLimit) { break }
        $results.Add($file)
      }

      if ($depth -lt $DepthLimit) {
        $dirs = @(
          Get-ChildItem -LiteralPath $path -Directory -ErrorAction SilentlyContinue |
            Where-Object { -not (($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -eq [IO.FileAttributes]::ReparsePoint) } |
            Sort-Object Name
        )
        foreach ($dir in $dirs) {
          $queue.Enqueue([ordered]@{ path = $dir.FullName; depth = ($depth + 1) })
        }
      }
    } catch {
    }
  }

  return $results.ToArray()
}

function Get-DirectoryProbe {
  param([string]$Root, [string]$Label)
  $probe = [ordered]@{
    source_kind = "machine_test"
    label = $Label
    path = $Root
    exists = $false
    readable = $false
    file_count_sampled = 0
    total_bytes_sampled = 0
    extension_counts = [ordered]@{}
    files = @()
    project_session_index = @()
    errors = @()
  }

  try {
    if (-not (Test-Path -LiteralPath $Root)) { return $probe }
    $probe.exists = $true
    $rootItem = Get-Item -LiteralPath $Root -ErrorAction Stop
    $probe.path = $rootItem.FullName

    $files = Get-BoundedFiles -Root $rootItem.FullName -DepthLimit $MaxDepth -FileLimit $MaxFilesPerRoot
    $probe.readable = $true
    foreach ($file in $files) {
      $relative = ConvertTo-RelativePath -Root $rootItem.FullName -Path $file.FullName
      $ext = ([IO.Path]::GetExtension($file.FullName)).ToLowerInvariant()
      if (-not $ext) { $ext = "[no_ext]" }
      if (-not $probe.extension_counts.Contains($ext)) { $probe.extension_counts[$ext] = 0 }
      $probe.extension_counts[$ext] += 1
      $format = Get-FileFormatSummary -Path $file.FullName
      $probe.files += [ordered]@{
        source_kind = "machine_test"
        relative_path = $relative
        bytes = $file.Length
        last_write_utc = $file.LastWriteTimeUtc.ToString("o")
        extension = $ext
        sensitive_name = (Test-SensitiveFileName $file.FullName)
        format = $format
      }
      $probe.file_count_sampled += 1
      $probe.total_bytes_sampled += $file.Length
    }

    $projectsDir = Join-Path $rootItem.FullName "projects"
    if (Test-Path -LiteralPath $projectsDir) {
      $projectDirs = @(Get-ChildItem -LiteralPath $projectsDir -Directory -ErrorAction SilentlyContinue | Select-Object -First 100)
      foreach ($dir in $projectDirs) {
        $sessions = @(Get-BoundedFiles -Root $dir.FullName -DepthLimit 2 -FileLimit 200 -Filter "*.jsonl" | Sort-Object LastWriteTimeUtc -Descending)
        if (-not $sessions.Count) { continue }
        $latest = $sessions[0]
        $shape = Get-JsonlShape -Path $latest.FullName -MaxLines 40
        $probe.project_session_index += [ordered]@{
          source_kind = "machine_test"
          project_name = $dir.Name
          session_count = $sessions.Count
          latest_session_file = $latest.Name
          latest_write_utc = $latest.LastWriteTimeUtc.ToString("o")
          total_bytes = [int64](($sessions | Measure-Object Length -Sum).Sum)
          latest_shape = $shape
          completion_status_inference = if ($shape.has_result_like_field) { "partial_result_fields_detected" } else { "not_inferred" }
          inference_warning = "Internal transcript formats may change; treat this as feasibility evidence, not a stable product contract."
        }
      }
    }
  } catch {
    $probe.errors += (Redact-Text $_.Exception.Message)
  }

  return $probe
}

function Get-WslCandidates {
  $roots = @()
  $wsl = Get-Command wsl.exe -ErrorAction SilentlyContinue
  if (-not $wsl) { return $roots }
  $list = Invoke-ProcessCapture -FilePath $wsl.Source -Arguments @("--list", "--quiet") -TimeoutSeconds 8
  if (-not $list.ok) { return $roots }
  $distros = @(
    ($list.stdout -replace [char]0, "") -split "\r?\n" |
      ForEach-Object { "$_".Trim() } |
      Where-Object { $_ -and $_ -notmatch '^docker-desktop' }
  )
  foreach ($distro in $distros) {
    $wslHomeResult = Invoke-ProcessCapture -FilePath $wsl.Source -Arguments @("-d", $distro, "--", "sh", "-lc", 'printf "%s" "$HOME"') -TimeoutSeconds 8
    if (-not $wslHomeResult.ok -or -not $wslHomeResult.stdout.Trim()) { continue }
    foreach ($name in @(".claude", ".claude-science", ".codex")) {
      $linuxPath = $wslHomeResult.stdout.TrimEnd() + "/" + $name
      $converted = Invoke-ProcessCapture -FilePath $wsl.Source -Arguments @("-d", $distro, "--", "wslpath", "-w", $linuxPath) -TimeoutSeconds 8
      if ($converted.ok -and $converted.stdout.Trim()) {
        $roots += [ordered]@{
          label = "wsl:${distro}:${name}"
          path = $converted.stdout.Trim()
        }
      }
    }
  }
  return $roots
}

function Get-DefaultTargetRoots {
  $roots = @()
  $roots += [ordered]@{ label = "project_root"; path = $ProjectRoot }
  if ($env:USERPROFILE) {
    $roots += [ordered]@{ label = "windows_user:.claude"; path = (Join-Path $env:USERPROFILE ".claude") }
    $roots += [ordered]@{ label = "windows_user:.claude-science"; path = (Join-Path $env:USERPROFILE ".claude-science") }
    $roots += [ordered]@{ label = "windows_user:.codex"; path = (Join-Path $env:USERPROFILE ".codex") }
  }
  if ($env:APPDATA) {
    $roots += [ordered]@{ label = "appdata:ClaudeScienceAssistant"; path = (Join-Path $env:APPDATA "ClaudeScienceAssistant") }
  }
  if ($env:LOCALAPPDATA) {
    $roots += [ordered]@{ label = "localappdata:ClaudeScienceAssistant"; path = (Join-Path $env:LOCALAPPDATA "ClaudeScienceAssistant") }
  }
  $roots += Get-WslCandidates
  return $roots
}

function Get-CliProbe {
  param([string]$Tool)
  $cmd = Get-Command $Tool -ErrorAction SilentlyContinue | Select-Object -First 1
  $probe = [ordered]@{
    source_kind = "machine_test"
    tool = $Tool
    found = $false
    path = ""
    version_probe = $null
    help_features = $null
    agent_smoke = $null
  }
  if (-not $cmd) { return $probe }
  $probe.found = $true
  $probe.path = [string]$cmd.Source

  $versionArgs = @("--version")
  if ($Tool -eq "python") { $versionArgs = @("--version") }
  if ($Tool -eq "node") { $versionArgs = @("--version") }
  if ($Tool -eq "git") { $versionArgs = @("--version") }
  if ($Tool -eq "docker") { $versionArgs = @("--version") }
  if ($Tool -eq "wsl") { $versionArgs = @("--version") }

  if ($ExternalCliProbeMode -ne "none") {
    $version = Invoke-ToolCapture -CommandInfo $cmd -Tool $Tool -Arguments $versionArgs -TimeoutSeconds 12
    $probe.version_probe = [ordered]@{
      ok = $version.ok
      exit_code = $version.exit_code
      duration_ms = $version.duration_ms
      stdout = (Redact-Text $version.stdout 500)
      stderr = (Redact-Text $version.stderr 500)
      output = (Redact-Text (($version.stdout + "`n" + $version.stderr).Trim()) 500)
    }
    $help = Invoke-ToolCapture -CommandInfo $cmd -Tool $Tool -Arguments @("--help") -TimeoutSeconds 12
    $helpText = ($help.stdout + "`n" + $help.stderr)
    $probe.help_features = [ordered]@{
      help_ok = $help.ok
      exit_code = $help.exit_code
      duration_ms = $help.duration_ms
      has_json_output_hint = [bool]($helpText -match '(?i)json|output-format')
      has_non_interactive_hint = [bool]($helpText -match '(?i)(--print|\s-p[, ]|exec|non.interactive|prompt)')
      has_workdir_hint = [bool]($helpText -match '(?i)(cwd|workdir|working.directory|directory)')
      has_permission_hint = [bool]($helpText -match '(?i)(permission|approval|sandbox|allow|deny)')
      has_export_hint = [bool]($helpText -match '(?i)(export|dump|archive)')
      has_sdk_hint = [bool]($helpText -match '(?i)(sdk|mcp|server)')
      has_hook_hint = [bool]($helpText -match '(?i)(hook|hooks)')
      has_api_hint = [bool]($helpText -match '(?i)(api|endpoint|base-url|url)')
      authentication_status = "not_checked_by_default"
    }
  }

  if ($ExternalCliProbeMode -eq "agent" -and ($Tool -eq "claude" -or $Tool -eq "codex")) {
    if ($Tool -eq "claude") {
      $probe.agent_smoke = Invoke-ToolCapture -CommandInfo $cmd -Tool $Tool -Arguments @("-p", "Return exactly CSA_PROBE_OK. Do not inspect or modify files.", "--output-format", "json") -TimeoutSeconds 60
    } elseif ($Tool -eq "codex") {
      $probe.agent_smoke = Invoke-ToolCapture -CommandInfo $cmd -Tool $Tool -Arguments @("exec", "Return exactly CSA_PROBE_OK. Do not inspect or modify files.") -TimeoutSeconds 60
    }
  }

  return $probe
}

function Get-ResourceSnapshot {
  $os = Get-CimInstance Win32_OperatingSystem -ErrorAction SilentlyContinue
  $cs = Get-CimInstance Win32_ComputerSystem -ErrorAction SilentlyContinue
  $cpu = Get-CimInstance Win32_Processor -ErrorAction SilentlyContinue | Select-Object -First 1
  $battery = @(Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue)
  $drives = @(
    Get-PSDrive -PSProvider FileSystem -ErrorAction SilentlyContinue |
      Where-Object { $null -ne $_.Free -and $null -ne $_.Used } |
      ForEach-Object {
        [ordered]@{
          name = $_.Name
          root = $_.Root
          free_gb = [math]::Round([double]$_.Free / 1GB, 1)
          used_gb = [math]::Round([double]$_.Used / 1GB, 1)
        }
      }
  )

  $gpu = [ordered]@{ nvidia_smi_found = $false; query_ok = $false; gpus = @(); error = "" }
  $nvidia = Get-Command nvidia-smi -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($nvidia) {
    $gpu.nvidia_smi_found = $true
    $query = Invoke-ProcessCapture -FilePath $nvidia.Source -Arguments @("--query-gpu=name,driver_version,memory.total,memory.used,temperature.gpu,power.draw", "--format=csv,noheader,nounits") -TimeoutSeconds 8
    $gpu.query_ok = $query.ok
    if ($query.ok) {
      $gpu.gpus = @(
        $query.stdout -split "\r?\n" |
          Where-Object { $_.Trim() } |
          ForEach-Object {
            $parts = @($_ -split "," | ForEach-Object { $_.Trim() })
            [ordered]@{
              name = if ($parts.Count -gt 0) { $parts[0] } else { "" }
              driver = if ($parts.Count -gt 1) { $parts[1] } else { "" }
              memory_total_mb = if ($parts.Count -gt 2) { $parts[2] } else { "" }
              memory_used_mb = if ($parts.Count -gt 3) { $parts[3] } else { "" }
              temperature_c = if ($parts.Count -gt 4) { $parts[4] } else { "" }
              power_draw_w = if ($parts.Count -gt 5) { $parts[5] } else { "" }
            }
          }
      )
    } else {
      $gpu.error = (Redact-Text (($query.stdout + "`n" + $query.stderr).Trim()) 500)
    }
  }

  return [ordered]@{
    source_kind = "machine_test"
    os = if ($os) {
      [ordered]@{
        caption = $os.Caption
        version = $os.Version
        build_number = $os.BuildNumber
        architecture = $os.OSArchitecture
        free_physical_memory_mb = [math]::Round([double]$os.FreePhysicalMemory / 1024, 1)
        total_visible_memory_mb = [math]::Round([double]$os.TotalVisibleMemorySize / 1024, 1)
      }
    } else { $null }
    computer = if ($cs) {
      [ordered]@{
        manufacturer = $cs.Manufacturer
        model = $cs.Model
        total_physical_memory_gb = [math]::Round([double]$cs.TotalPhysicalMemory / 1GB, 1)
      }
    } else { $null }
    cpu = if ($cpu) {
      [ordered]@{
        name = $cpu.Name
        cores = $cpu.NumberOfCores
        logical_processors = $cpu.NumberOfLogicalProcessors
      }
    } else { $null }
    disks = $drives
    gpu = $gpu
    battery = @(
      $battery | ForEach-Object {
        [ordered]@{
          name = $_.Name
          estimated_charge_remaining = $_.EstimatedChargeRemaining
          battery_status = $_.BatteryStatus
        }
      }
    )
  }
}

function Get-EnvironmentSnapshot {
  $process = Get-Process -Id $PID -ErrorAction SilentlyContinue
  $identityName = [Security.Principal.WindowsIdentity]::GetCurrent().Name
  $principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
  $isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  $wslCommand = Get-Command wsl.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  $dockerCommand = Get-Command docker -ErrorAction SilentlyContinue | Select-Object -First 1
  $runtimeHints = @("native_windows", "local_cli")
  if ($wslCommand) { $runtimeHints += "wsl_available" }
  if ($dockerCommand) { $runtimeHints += "docker_cli_available" }

  return [ordered]@{
    source_kind = "machine_test"
    os_platform = [Environment]::OSVersion.Platform.ToString()
    os_version = [Environment]::OSVersion.VersionString
    process_architecture = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
    current_user = $identityName
    is_elevated = $isAdmin
    shell = [ordered]@{
      process_name = if ($process) { $process.ProcessName } else { "unknown" }
      executable = if ($process) { $process.Path } else { "" }
      powershell_version = $PSVersionTable.PSVersion.ToString()
    }
    working_directory = (Get-Location).Path
    project_root = $ProjectRoot
    runtime_inference = [ordered]@{
      hints = $runtimeHints
      note = "Runtime classification is inferred from local command availability and the user-provided TargetRuntime parameter."
    }
  }
}

function Get-WindowsPortSnapshot {
  param([int[]]$Ports)
  $result = [ordered]@{}
  foreach ($port in $Ports) {
    $result[[string]$port] = @(
      Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue |
        ForEach-Object {
          $process = Get-CimInstance Win32_Process -Filter "ProcessId=$($_.OwningProcess)" -ErrorAction SilentlyContinue
          [ordered]@{
            source_kind = "machine_test"
            address = $_.LocalAddress
            port = $_.LocalPort
            pid = $_.OwningProcess
            process = if ($process) { $process.Name } else { "unknown" }
            command = if ($process) { Redact-Text $process.CommandLine 500 } else { "" }
          }
        }
    )
  }
  return $result
}

function Get-ExistingCsaStatusProbe {
  $statusScript = Join-Path $ProjectRoot "scripts\status-probe.ps1"
  if (-not (Test-Path -LiteralPath $statusScript)) {
    return [ordered]@{ source_kind = "machine_test"; available = $false; result = $null; error = "" }
  }
  $result = Invoke-ProcessCapture -FilePath "powershell" -Arguments @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $statusScript, "-ProjectRoot", $ProjectRoot) -TimeoutSeconds 45
  if (-not $result.ok) {
    return [ordered]@{ source_kind = "machine_test"; available = $true; result = $null; error = (Redact-Text (($result.stdout + "`n" + $result.stderr).Trim()) 1000) }
  }
  try {
    return [ordered]@{ source_kind = "machine_test"; available = $true; result = ($result.stdout | ConvertFrom-Json -ErrorAction Stop); error = "" }
  } catch {
    return [ordered]@{ source_kind = "machine_test"; available = $true; result = $null; error = "status_probe_json_parse_failed" }
  }
}

function New-FeasibilityDecision {
  param($StorageRoots, $CliProbes, $ResourceSnapshot)
  $existingRoots = @($StorageRoots | Where-Object { $_.exists })
  $readableRoots = @($StorageRoots | Where-Object { $_.readable })
  $sessionRoots = @($StorageRoots | Where-Object { @($_.project_session_index).Count -gt 0 })
  $structuredFiles = @(
    $StorageRoots | ForEach-Object { $_.files } | Where-Object {
      $_.format.format -eq "json" -or $_.format.format -eq "jsonl" -or $_.format.format -like "sqlite*"
    }
  )
  $agentTools = @($CliProbes | Where-Object { $_.tool -in @("claude", "codex") -and $_.found -and $_.version_probe -and $_.version_probe.ok })
  $anyCli = @($CliProbes | Where-Object { $_.found -and ((-not $_.version_probe) -or $_.version_probe.ok) })
  $blockers = New-Object System.Collections.Generic.List[string]
  $next = New-Object System.Collections.Generic.List[string]

  if (-not $existingRoots.Count) { $blockers.Add("No target local storage roots were found.") }
  if (-not $readableRoots.Count) { $blockers.Add("No target local storage roots were readable.") }
  if (-not $structuredFiles.Count) { $blockers.Add("No structured JSON/JSONL/SQLite-like files were detected in the sampled roots.") }
  if (-not $agentTools.Count) { $blockers.Add("No callable external CLI agent was detected among claude/codex.") }

  $next.Add("POST /api/probes/local-observability/runs")
  $next.Add("GET /api/probes/local-observability/runs/:runId")
  $next.Add("GET /api/projects/discovered")
  $next.Add("GET /api/projects/:id/local-sessions")
  $next.Add("GET /api/external-tools")
  $next.Add("POST /api/external-runs")
  $next.Add("GET /api/resource-snapshots/latest")

  $overall = "conditional"
  if ($existingRoots.Count -and $readableRoots.Count -and $structuredFiles.Count -and $agentTools.Count) {
    $overall = "feasible"
  } elseif (-not $existingRoots.Count -and -not $agentTools.Count) {
    $overall = "not_ready"
  }

  return [ordered]@{
    overall = $overall
    local_storage_found = [bool]$existingRoots.Count
    local_storage_readable = [bool]$readableRoots.Count
    structured_state_detected = [bool]$structuredFiles.Count
    session_index_detected = [bool]$sessionRoots.Count
    external_cli_agent_detected = [bool]$agentTools.Count
    cli_any_detected = [bool]$anyCli.Count
    resource_snapshot_ok = [bool]$ResourceSnapshot
    blockers_or_gaps = @($blockers)
    recommended_first_interfaces = @($next)
    required_compensating_controls = @(
      "Read-only default scan mode",
      "Sensitive filename/content redaction",
      "Per-project authorization before agent execution",
      "External CLI command allowlist",
      "Timeout, stdout/stderr capture, and audit logs",
      "Billable agent smoke tests disabled unless explicitly requested"
    )
  }
}

function New-FeasibilityAssessment {
  param($Decision, $StorageRoots, $CliProbes, $ResourceSnapshot, [string]$ProbeMode)

  $readableRoots = @($StorageRoots | Where-Object { $_.readable })
  $sessionRoots = @($StorageRoots | Where-Object { @($_.project_session_index).Count -gt 0 })
  $structuredFiles = @(
    $StorageRoots | ForEach-Object { $_.files } | Where-Object {
      $_.format.format -eq "json" -or $_.format.format -eq "jsonl" -or $_.format.format -like "sqlite*"
    }
  )
  $callableAgents = @($CliProbes | Where-Object { $_.tool -in @("claude", "codex") -and $_.found -and $_.version_probe -and $_.version_probe.ok })
  $codexProbe = @($CliProbes | Where-Object { $_.tool -eq "codex" } | Select-Object -First 1)

  $verified = @()
  if ($readableRoots.Count) {
    $verified += [ordered]@{
      id = "local_roots_readable"
      capability = "Target local storage roots can be located and read."
      source_kind = "machine_test"
      evidence = "$($readableRoots.Count) readable root(s)."
    }
  }
  if ($structuredFiles.Count) {
    $verified += [ordered]@{
      id = "structured_state_detected"
      capability = "Structured state files are present in the sampled roots."
      source_kind = "machine_test"
      evidence = "$($structuredFiles.Count) JSON/JSONL/SQLite-like sampled file(s)."
    }
  }
  if ($sessionRoots.Count) {
    $verified += [ordered]@{
      id = "session_index_detected"
      capability = "Project/session indexes can be inferred from local files."
      source_kind = "machine_test"
      evidence = "$($sessionRoots.Count) root(s) contain project session index candidates."
    }
  }
  if ($callableAgents.Count) {
    $verified += [ordered]@{
      id = "external_cli_agent_callable"
      capability = "At least one external CLI Agent is callable by version probe."
      source_kind = "machine_test"
      evidence = (@($callableAgents | ForEach-Object { "$($_.tool): $($_.version_probe.output)" }) -join "; ")
    }
  }
  if ($ResourceSnapshot) {
    $verified += [ordered]@{
      id = "resource_snapshot_collected"
      capability = "Machine resource status can be collected."
      source_kind = "machine_test"
      evidence = "OS/CPU/memory/disk/GPU/battery probes were attempted with secret redaction."
    }
  }

  $unverified = @(
    [ordered]@{
      id = "official_docs_contract"
      capability = "Official documentation confirmation for storage format, export, SDK, hooks, and API contracts."
      source_kind = "official_docs"
      status = "not_checked_by_probe"
      reason = "This local probe is intentionally offline; official docs must be reviewed separately before productizing stable contracts."
    },
    [ordered]@{
      id = "full_task_completion_status"
      capability = "Reliable task completion status and semantic summary from transcripts."
      source_kind = "inference"
      status = "not_proven"
      reason = "The probe reads transcript shape only; it does not parse or expose full private conversation content."
    }
  )
  if ($ProbeMode -ne "agent") {
    $unverified += [ordered]@{
      id = "authenticated_agent_smoke_run"
      capability = "Authenticated non-mutating external Agent smoke run."
      source_kind = "machine_test"
      status = "not_run"
      reason = "ExternalCliProbeMode is '$ProbeMode'; billable/interactive Agent execution is disabled by default."
    }
  }
  if ($codexProbe -and $codexProbe.found -and $codexProbe.version_probe -and -not $codexProbe.version_probe.ok) {
    $unverified += [ordered]@{
      id = "codex_cli_callable"
      capability = "Codex CLI is callable on this machine."
      source_kind = "machine_test"
      status = "failed"
      reason = $codexProbe.version_probe.output
    }
  }

  $riskLevel = "medium"
  if ($Decision.overall -eq "conditional") { $riskLevel = "high" }
  if ($Decision.overall -eq "not_ready") { $riskLevel = "critical" }

  $risks = @(
    [ordered]@{
      id = "internal_format_instability"
      level = "medium"
      source_kind = "inference"
      evidence = "Local project/session indexes may rely on internal JSONL layout; treat as feasibility evidence, not stable product contract."
      mitigation = "Prefer official export, JSON output, SDK, hook, or API when available; keep internal parsing behind adapters."
    },
    [ordered]@{
      id = "privacy_leakage"
      level = "high"
      source_kind = "inference"
      evidence = "Local transcript and config directories may contain private prompts, tokens, or project data."
      mitigation = "Shape-only sampling, sensitive filename skip list, redaction, local-only reports, and UI-side escaping."
    },
    [ordered]@{
      id = "external_cli_side_effects"
      level = "high"
      source_kind = "inference"
      evidence = "Agent CLIs may read/write files or call paid services if fully invoked."
      mitigation = "Default to version/help probes; require explicit project authorization, command allowlist, timeout, and audit logs."
    },
    [ordered]@{
      id = "official_contract_gap"
      level = "medium"
      source_kind = "official_docs"
      evidence = "No official documentation is fetched or validated by the local probe."
      mitigation = "Before product release, verify each connector against official docs and annotate the report with official-doc evidence."
    }
  )

  $remediations = @(
    [ordered]@{
      condition = "Local files are unreadable or encrypted."
      action = "Use official export/SDK/API/hooks, or request user-selected workspace roots instead of scanning private app storage."
    },
    [ordered]@{
      condition = "CLI exists but cannot execute."
      action = "Record the exact launch error, prefer another callable Agent, or repair the CLI installation/alias outside the probe."
    },
    [ordered]@{
      condition = "Environment cannot install dependencies."
      action = "Run tasks through Docker/WSL/remote Worker, but keep project authorization and command allowlists."
    },
    [ordered]@{
      condition = "Internal transcript parsing is too unstable."
      action = "Restrict CSA to metadata and use official export or CLI JSON output for deeper state."
    }
  )

  $officialInterfaceCandidates = @(
    $CliProbes | Where-Object { $_.found } | ForEach-Object {
      [ordered]@{
        tool = $_.tool
        source_kind = "machine_test"
        official_docs_status = "not_checked_by_probe"
        version_ok = if ($_.version_probe) { $_.version_probe.ok } else { $null }
        has_json_output_hint = if ($_.help_features) { $_.help_features.has_json_output_hint } else { $null }
        has_export_hint = if ($_.help_features) { $_.help_features.has_export_hint } else { $null }
        has_sdk_hint = if ($_.help_features) { $_.help_features.has_sdk_hint } else { $null }
        has_hook_hint = if ($_.help_features) { $_.help_features.has_hook_hint } else { $null }
        has_api_hint = if ($_.help_features) { $_.help_features.has_api_hint } else { $null }
      }
    }
  )

  $acceptance = @(
    [ordered]@{
      id = "read_local_project_session_state"
      requirement = "Can answer whether local project/session state is readable."
      status = if ($Decision.local_storage_readable -and $Decision.session_index_detected) { "passed" } else { "incomplete" }
      source_kind = "machine_test"
      evidence = "local_storage_readable=$($Decision.local_storage_readable); session_index_detected=$($Decision.session_index_detected)"
    },
    [ordered]@{
      id = "call_external_cli_agent"
      requirement = "Can answer whether an external CLI Agent can be called."
      status = if ($Decision.external_cli_agent_detected) { "passed" } else { "incomplete" }
      source_kind = "machine_test"
      evidence = "callable_agents=$(@($callableAgents | ForEach-Object { $_.tool }) -join ',')"
    },
    [ordered]@{
      id = "permissions_and_security_boundary"
      requirement = "Can answer which permissions and safety boundaries are required."
      status = "passed"
      source_kind = "inference"
      evidence = "safety model, controls, risks, and remediations are embedded in the report."
    },
    [ordered]@{
      id = "next_backend_interfaces"
      requirement = "Can provide an executable next-stage backend interface list."
      status = if (@($Decision.recommended_first_interfaces).Count -gt 0) { "passed" } else { "incomplete" }
      source_kind = "inference"
      evidence = "$(@($Decision.recommended_first_interfaces).Count) interface(s) proposed."
    },
    [ordered]@{
      id = "evidence_source_classification"
      requirement = "Findings distinguish machine tests, official-doc checks, and inference."
      status = "passed"
      source_kind = "inference"
      evidence = "verified_capabilities, unverified_capabilities, risks, and interface candidates carry source_kind fields."
    }
  )

  return [ordered]@{
    risk_level = $riskLevel
    recommend_next_stage = ($Decision.overall -eq "feasible")
    next_stage = if ($Decision.overall -eq "feasible") { "build_minimal_csa_worker_and_backend_interfaces" } elseif ($Decision.overall -eq "conditional") { "fix_blockers_then_repeat_probe" } else { "do_not_build_backend_until_probe_conditions_pass" }
    verified_capabilities = $verified
    unverified_capabilities = $unverified
    risk_register = $risks
    recommended_remediations = $remediations
    official_interface_priority = [ordered]@{
      policy = "Prefer official export / CLI JSON output / SDK / hooks / API over private local file parsing. Local internal files are acceptable for feasibility testing only."
      candidates = $officialInterfaceCandidates
    }
    acceptance_matrix = $acceptance
  }
}

function Write-MarkdownReport {
  param($Report, [string]$Path)
  $lines = New-Object System.Collections.Generic.List[string]
  $lines.Add("# CSA Local Observability Feasibility Report")
  $lines.Add("")
  $lines.Add("- Generated: $($Report.generated_at)")
  $lines.Add("- Target software: $($Report.target.software_name)")
  $lines.Add("- Target runtime: $($Report.target.runtime)")
  $lines.Add("- Test machine: $($Report.target.test_machine)")
  $lines.Add("- Overall: $($Report.feasibility.overall)")
  $lines.Add("- Risk level: $($Report.assessment.risk_level)")
  $lines.Add("- Recommend next stage: $($Report.assessment.recommend_next_stage)")
  $lines.Add("- Project root: ``$($Report.project_root)``")
  $lines.Add("- External CLI probe mode: $($Report.external_cli_probe_mode)")
  $lines.Add("- Secret values included: false")
  $lines.Add("- Current user: $($Report.environment.current_user)")
  $lines.Add("- Shell: $($Report.environment.shell.process_name) $($Report.environment.shell.powershell_version)")
  $lines.Add("- Working directory: ``$($Report.environment.working_directory)``")
  $lines.Add("")
  $lines.Add("## Decision")
  $lines.Add("")
  foreach ($name in @("local_storage_found", "local_storage_readable", "structured_state_detected", "session_index_detected", "external_cli_agent_detected", "resource_snapshot_ok")) {
    $lines.Add("- ${name}: $($Report.feasibility.$name)")
  }
  if (@($Report.feasibility.blockers_or_gaps).Count) {
    $lines.Add("")
    $lines.Add("## Blockers Or Gaps")
    $lines.Add("")
    foreach ($gap in $Report.feasibility.blockers_or_gaps) {
      $lines.Add("- $gap")
    }
  }
  $lines.Add("")
  $lines.Add("## Verified Capabilities")
  $lines.Add("")
  foreach ($capability in $Report.assessment.verified_capabilities) {
    $lines.Add("- [$($capability.source_kind)] $($capability.id): $($capability.capability)")
    $lines.Add("  - evidence: $($capability.evidence)")
  }
  $lines.Add("")
  $lines.Add("## Unverified Capabilities")
  $lines.Add("")
  foreach ($capability in $Report.assessment.unverified_capabilities) {
    $lines.Add("- [$($capability.source_kind)] $($capability.id): $($capability.capability)")
    $lines.Add("  - status: $($capability.status)")
    $lines.Add("  - reason: $($capability.reason)")
  }
  $lines.Add("")
  $lines.Add("## Risk Register")
  $lines.Add("")
  foreach ($risk in $Report.assessment.risk_register) {
    $lines.Add("- [$($risk.level)][$($risk.source_kind)] $($risk.id): $($risk.evidence)")
    $lines.Add("  - mitigation: $($risk.mitigation)")
  }
  $lines.Add("")
  $lines.Add("## Recommended Remediations")
  $lines.Add("")
  foreach ($remediation in $Report.assessment.recommended_remediations) {
    $lines.Add("- $($remediation.condition)")
    $lines.Add("  - action: $($remediation.action)")
  }
  $lines.Add("")
  $lines.Add("## Acceptance Matrix")
  $lines.Add("")
  foreach ($item in $Report.assessment.acceptance_matrix) {
    $lines.Add("- $($item.id): $($item.status)")
    $lines.Add("  - requirement: $($item.requirement)")
    $lines.Add("  - evidence: [$($item.source_kind)] $($item.evidence)")
  }
  $lines.Add("")
  $lines.Add("## Official Interface Priority")
  $lines.Add("")
  $lines.Add($Report.assessment.official_interface_priority.policy)
  $lines.Add("")
  foreach ($candidate in $Report.assessment.official_interface_priority.candidates) {
    $lines.Add("- $($candidate.tool): docs=$($candidate.official_docs_status), version_ok=$($candidate.version_ok), json=$($candidate.has_json_output_hint), export=$($candidate.has_export_hint), sdk=$($candidate.has_sdk_hint), hooks=$($candidate.has_hook_hint), api=$($candidate.has_api_hint)")
  }
  $lines.Add("")
  $lines.Add("## Local Storage Roots")
  $lines.Add("")
  foreach ($root in $Report.local_storage_roots) {
    $lines.Add("- $($root.label): exists=$($root.exists), readable=$($root.readable), files_sampled=$($root.file_count_sampled), sessions=$(@($root.project_session_index).Count)")
  }
  $lines.Add("")
  $lines.Add("## External CLI Tools")
  $lines.Add("")
  foreach ($cli in $Report.external_cli_tools) {
    $versionText = ""
    if ($cli.version_probe) { $versionText = $cli.version_probe.output }
    $versionOk = if ($cli.version_probe) { $cli.version_probe.ok } else { "not_run" }
    $lines.Add("- $($cli.tool): found=$($cli.found), version_ok=$versionOk, path=``$($cli.path)``")
    if ($versionText) {
      $lines.Add("  - version: $versionText")
    }
  }
  $lines.Add("")
  $lines.Add("## Recommended First Interfaces")
  $lines.Add("")
  foreach ($iface in $Report.feasibility.recommended_first_interfaces) {
    $lines.Add("- ``$iface``")
  }
  $lines.Add("")
  $lines.Add("## Safety Notes")
  $lines.Add("")
  foreach ($control in $Report.feasibility.required_compensating_controls) {
    $lines.Add("- $control")
  }
  $lines.Add("")
  $lines.Add("Detailed, machine-readable evidence is in ``feasibility_report.json``.")
  Set-Content -LiteralPath $Path -Value $lines.ToArray() -Encoding UTF8
}

$rootSpecs = @()
if ($TargetRoots.Count) {
  foreach ($root in $TargetRoots) {
    $rootSpecs += [ordered]@{ label = "custom"; path = $root }
  }
} else {
  $rootSpecs = Get-DefaultTargetRoots
}

$localRoots = @()
foreach ($spec in $rootSpecs) {
  $localRoots += Get-DirectoryProbe -Root $spec.path -Label $spec.label
}

$cliProbes = @()
foreach ($tool in $CliTools) {
  $cliProbes += Get-CliProbe -Tool $tool
}

$environment = Get-EnvironmentSnapshot
$resources = Get-ResourceSnapshot
$csaStatus = Get-ExistingCsaStatusProbe
$ports = if ($IncludeWindowsPorts) { Get-WindowsPortSnapshot -Ports @(9876, 9877, 8765, 8766, 443) } else { $null }
$decision = New-FeasibilityDecision -StorageRoots $localRoots -CliProbes $cliProbes -ResourceSnapshot $resources
$assessment = New-FeasibilityAssessment -Decision $decision -StorageRoots $localRoots -CliProbes $cliProbes -ResourceSnapshot $resources -ProbeMode $ExternalCliProbeMode

$report = [ordered]@{
  schema_version = 1
  generated_at = (Get-Date).ToUniversalTime().ToString("o")
  target = [ordered]@{
    software_name = $TargetSoftwareName
    runtime = $TargetRuntime
    test_machine = $TestMachine
    allowed_scope = "Read-only scan unless explicitly authorized; no destructive commands, installs, uploads, or full private transcript output."
  }
  project_root = $ProjectRoot
  output_dir = (Resolve-Path -LiteralPath $OutputDir).Path
  external_cli_probe_mode = $ExternalCliProbeMode
  safety = [ordered]@{
    read_only = $true
    secret_values_included = $false
    sensitive_name_pattern = $SensitiveNamePattern
    sampled_content_policy = "JSON/JSONL shape only; values are not emitted; sensitive filenames are never content-sampled."
    billable_agent_calls_require_mode_agent = $true
  }
  feasibility = $decision
  assessment = $assessment
  environment = $environment
  local_storage_roots = $localRoots
  external_cli_tools = $cliProbes
  resources = $resources
  csa_runtime_status_probe = $csaStatus
  windows_ports = $ports
}

$jsonPath = Join-Path $OutputDir "feasibility_report.json"
$mdPath = Join-Path $OutputDir "feasibility_report.md"
$json = $report | ConvertTo-Json -Depth 30
Set-Content -LiteralPath $jsonPath -Value $json -Encoding UTF8
Write-MarkdownReport -Report $report -Path $mdPath

if ($JsonToStdout) {
  $json
} else {
  Write-Host "CSA feasibility probe completed."
  Write-Host "Overall: $($decision.overall)"
  Write-Host "JSON: $jsonPath"
  Write-Host "Markdown: $mdPath"
}
