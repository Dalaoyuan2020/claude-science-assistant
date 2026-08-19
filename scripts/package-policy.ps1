Set-StrictMode -Version Latest

function Get-CsaGitSourceState {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$ProjectDir
  )

  if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    throw "Git is required to create a traceable CSA package."
  }

  $commitOutput = @(& git -C $ProjectDir rev-parse HEAD 2>$null)
  $commitSucceeded = $?
  $commit = ($commitOutput | Select-Object -First 1)
  if (-not $commitSucceeded -or -not $commit) {
    throw "Unable to resolve the source commit for packaging."
  }

  $treeOutput = @(& git -C $ProjectDir rev-parse 'HEAD^{tree}' 2>$null)
  $treeSucceeded = $?
  $tree = ($treeOutput | Select-Object -First 1)
  if (-not $treeSucceeded -or -not $tree) {
    throw "Unable to resolve the source tree for packaging."
  }

  $status = @(& git -C $ProjectDir status --porcelain=v1 --untracked-files=all 2>$null)
  $statusSucceeded = $?
  if (-not $statusSucceeded) {
    throw "Unable to inspect the source worktree for packaging."
  }

  [pscustomobject]@{
    Commit = ([string]$commit).Trim()
    Tree = ([string]$tree).Trim()
    Dirty = ($status.Count -gt 0)
  }
}

function Assert-CsaPackageSourcePolicy {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [psobject]$SourceState,
    [Parameter(Mandatory = $true)]
    [ValidateSet("debug", "release")]
    [string]$Profile,
    [bool]$AllowDirtySource = $false
  )

  if ([string]::IsNullOrWhiteSpace([string]$SourceState.Commit) -or
      [string]::IsNullOrWhiteSpace([string]$SourceState.Tree)) {
    throw "Packaging requires an identified Git commit and tree."
  }
  if ($Profile -eq "release" -and $AllowDirtySource) {
    throw "-AllowDirtySource is a debug-only escape hatch and cannot be used for release packages."
  }
  if ([bool]$SourceState.Dirty -and $Profile -eq "release") {
    throw "Release packaging requires a clean Git worktree. Commit or remove local changes first."
  }
  if ([bool]$SourceState.Dirty -and -not $AllowDirtySource) {
    throw "Packaging refused a dirty Git worktree. Debug-only experiments must opt in with -AllowDirtySource."
  }
}

function Write-CsaUtf8NoBom {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,
    [Parameter(Mandatory = $true)]
    [AllowEmptyString()]
    [string]$Content
  )

  $encoding = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Assert-CsaUtf8NoBom {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path
  )

  $bytes = [System.IO.File]::ReadAllBytes($Path)
  if ($bytes.Length -ge 3 -and
      $bytes[0] -eq 0xEF -and
      $bytes[1] -eq 0xBB -and
      $bytes[2] -eq 0xBF) {
    throw "UTF-8 BOM is not allowed in package JSON: $Path"
  }
}
