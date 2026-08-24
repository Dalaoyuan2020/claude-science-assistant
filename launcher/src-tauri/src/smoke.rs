use super::{
    background_command, command_output_with_stdin_timeout, command_output_with_timeout,
    current_status, get_claude_url_impl, output_text, preferred_distro, project_root,
    runtime_identity_from_health,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Output;
use std::thread;
use std::time::{Duration, Instant};

const DISTRO_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(550);
const ALLOW_PROBE_TIMEOUT: Duration = Duration::from_millis(2_200);
const WINDOWS_PORT_PROBE_TIMEOUT: Duration = Duration::from_millis(350);
const WINDOWS_PROCESS_PROBE_TIMEOUT: Duration = Duration::from_millis(1_600);
const BRIDGE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PAINT_BUDGET: Duration = Duration::from_secs(3);
const OPEN_BUDGET: Duration = Duration::from_secs(8);
const ALLOW_OPEN_INPUTS: [&str; 2] = ["claudeRunning", "windowsBridgePid"];

const ALLOW_PROBE_SHELL: &str = r#"
set -u
state_root="${CSA_STATE_ROOT:-$HOME/.local/share/csa}"
legacy_root="$HOME/.local/share/claude-science-api-bridge"

listener_pids() {
  local port="$1"
  ss -ltnp "sport = :$port" 2>/dev/null \
    | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u
}

process_start_ticks() {
  local pid="$1" payload suffix
  payload="$(<"/proc/$pid/stat")" 2>/dev/null || return 1
  suffix="${payload##*) }"
  set -- $suffix
  case "${20:-}" in ''|*[!0-9]*) return 1;; esac
  printf '%s\n' "${20}"
}

linux_user="$(id -un 2>/dev/null || true)"
case "$linux_user" in
  ''|*[!A-Za-z0-9._-]*) echo CSA_ALLOW_DEFAULT_USER_INVALID >&2; exit 68;;
esac

command -v ss >/dev/null 2>&1 || { echo CSA_ALLOW_SS_MISSING >&2; exit 69; }
listeners_8765="$(ss -H -ltn 'sport = :8765' 2>/dev/null)" \
  || { echo CSA_ALLOW_SS_FAILED >&2; exit 69; }
listeners_8766="$(ss -H -ltn 'sport = :8766' 2>/dev/null)" \
  || { echo CSA_ALLOW_SS_FAILED >&2; exit 69; }
listener_probe_ok=true

runtime_present=false
current_root="$state_root/runtime/claude-science/current"
current_target="$(readlink -f "$current_root" 2>/dev/null || true)"
case "$current_target" in
  "$state_root"/runtime/claude-science/versions/*)
    if [ -x "$current_root/claude-science" ] && [ -f "$current_root/runtime-manifest.json" ]; then
      runtime_present=true
    fi
    ;;
esac

pids_8765="$(listener_pids 8765 || true)"
pids_8766="$(listener_pids 8766 || true)"
count_8765="$(printf '%s\n' "$pids_8765" | sed '/^$/d' | wc -l)"
count_8766="$(printf '%s\n' "$pids_8766" | sed '/^$/d' | wc -l)"
pid_8765=null
pid_8766=null
[ "$count_8765" = 1 ] && pid_8765="$(printf '%s\n' "$pids_8765" | sed '/^$/d' | head -1)"
[ "$count_8766" = 1 ] && pid_8766="$(printf '%s\n' "$pids_8766" | sed '/^$/d' | head -1)"

listener_present=false
if [ -n "$listeners_8765" ] || [ -n "$listeners_8766" ]; then listener_present=true; fi
claude_running=false
daemon_state=stopped
claude_pid=null

if [ "$listener_present" = true ]; then
  daemon_state=partial_listener
  if [ "$count_8765" = 1 ] && [ "$count_8766" = 1 ] && [ "$pid_8765" = "$pid_8766" ]; then
    candidate_pid="$pid_8765"
    executable="$(readlink "/proc/$candidate_pid/exe" 2>/dev/null || true)"
    argv=()
    mapfile -d '' -t argv <"/proc/$candidate_pid/cmdline" 2>/dev/null || true
    argv0_real="$(readlink -f -- "${argv[0]:-}" 2>/dev/null || true)"
    start_before="$(process_start_ticks "$candidate_pid" 2>/dev/null || true)"
    managed=false
    case "$executable" in
      "$state_root"/runtime/claude-science/versions/*/claude-science|"$state_root"/runtime/claude-science/patched/*/claude-science|"$legacy_root"/patched/claude-science)
        managed=true
        ;;
    esac
    start_after="$(process_start_ticks "$candidate_pid" 2>/dev/null || true)"
    if [ "$managed" = true ] \
      && [ -n "$start_before" ] \
      && [ "$start_before" = "$start_after" ] \
      && [ "$argv0_real" = "$executable" ] \
      && [ "${argv[1]:-}" = serve ]; then
      claude_running=true
      daemon_state=managed_ready
      claude_pid="$candidate_pid"
    else
      daemon_state=unverified_listener
    fi
  elif [ "$count_8765" -gt 1 ] || [ "$count_8766" -gt 1 ] \
    || { [ "$count_8765" = 1 ] && [ "$count_8766" = 1 ] && [ "$pid_8765" != "$pid_8766" ]; }; then
    daemon_state=unverified_listener
  fi
fi

control_socket_present=false
[ -S "$HOME/.claude-science/daemon.sock" ] && control_socket_present=true

printf '{'
printf '"linux_user":"%s",' "$linux_user"
printf '"runtime_present":%s,' "$runtime_present"
printf '"claude_running":%s,' "$claude_running"
printf '"claude_pid":%s,' "$claude_pid"
printf '"listener_present":%s,' "$listener_present"
printf '"pid_8765":%s,' "$pid_8765"
printf '"pid_8766":%s,' "$pid_8766"
printf '"daemon_state":"%s",' "$daemon_state"
printf '"listener_probe_ok":%s,' "$listener_probe_ok"
printf '"control_socket_present":%s' "$control_socket_present"
printf '}\n'
"#;

const BRIDGE_PROBE_PYTHON: &str = r#"
import hashlib
import http.client
import json
import os
import re
import socket
import subprocess
from pathlib import Path
from urllib.parse import quote

def emit(**values):
    values["secrets_included"] = False
    print(json.dumps(values, separators=(",", ":")))

def request(path, timeout):
    connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=timeout)
    try:
        connection.request("GET", path, headers={"Connection": "close"})
        response = connection.getresponse()
        return response.status, response.read(1024 * 1024)
    finally:
        connection.close()

try:
    health_status, health_body = request("/health", 1.5)
except (TimeoutError, socket.timeout):
    emit(error_code="bridge.health_timeout", health_status=None,
         models_status=None, models_count=0, models_valid=False,
         identity_current=False, runtime_identity=None)
    raise SystemExit(0)
except Exception:
    emit(error_code="bridge.health_unreachable", health_status=None,
         models_status=None, models_count=0, models_valid=False,
         identity_current=False, runtime_identity=None)
    raise SystemExit(0)

if health_status != 200:
    emit(error_code="bridge.health_http", health_status=health_status,
         models_status=None, models_count=0, models_valid=False,
         identity_current=False, runtime_identity=None)
    raise SystemExit(0)

try:
    health = json.loads(health_body.decode("utf-8"))
except Exception:
    emit(error_code="bridge.health_invalid", health_status=health_status,
         models_status=None, models_count=0, models_valid=False,
         identity_current=False, runtime_identity=None)
    raise SystemExit(0)

if not isinstance(health, dict):
    emit(error_code="bridge.health_invalid", health_status=health_status,
         models_status=None, models_count=0, models_valid=False,
         identity_current=False, runtime_identity=None)
    raise SystemExit(0)

identity = health.get("runtime_identity")
identity_current = False
try:
    current_proxy = Path.home() / ".local" / "share" / "csa" / "runtime" / "bridge" / "current" / "proxy.py"
    source_path = Path(str((identity or {}).get("sourcePath") or ""))
    source_hash = hashlib.sha256(current_proxy.read_bytes()).hexdigest()
    identity_pid = int((identity or {}).get("pid") or 0)
    sockets = subprocess.run(
        ["ss", "-H", "-ltnp", "sport = :9876"],
        capture_output=True, text=True, timeout=0.8, check=False,
    )
    listener_pids = set(int(value) for value in re.findall(r"pid=(\d+)", sockets.stdout))
    argv = (Path("/proc") / str(identity_pid) / "cmdline").read_bytes().split(b"\0")
    argv_paths = []
    for item in argv:
        try:
            value = item.decode("utf-8")
            if value.endswith("proxy.py"):
                argv_paths.append(Path(value).resolve())
        except Exception:
            pass
    identity_current = (
        current_proxy.resolve() == source_path.resolve()
        and str((identity or {}).get("sourceSha256") or "").casefold() == source_hash.casefold()
        and listener_pids == {identity_pid}
        and current_proxy.resolve() in argv_paths
    )
except Exception:
    identity_current = False

config_path = Path.home() / ".claude-science" / "proxy" / "config.json"
try:
    config = json.loads(config_path.read_text(encoding="utf-8")) if config_path.exists() else {}
    if not isinstance(config, dict):
        config = {}
except Exception:
    config = {}

mode = str(config.get("proxy_auth_mode") or "optional").strip().lower()
token = str(config.get("proxy_auth_token") or "").strip()
try:
    bridge_pid = int((identity or {}).get("pid") or 0)
    environ = (Path("/proc") / str(bridge_pid) / "environ").read_bytes()
    effective_env = {}
    for item in environ.split(b"\0"):
        if b"=" in item:
            key, value = item.split(b"=", 1)
            effective_env[key.decode("utf-8", "ignore")] = value.decode("utf-8", "ignore")
    if effective_env.get("PROXY_AUTH_MODE"):
        mode = effective_env["PROXY_AUTH_MODE"].strip().lower()
    if effective_env.get("PROXY_AUTH_TOKEN"):
        token = effective_env["PROXY_AUTH_TOKEN"].strip()
except Exception:
    pass

health_mode = str(health.get("proxy_auth_mode") or "optional").strip().lower()
health_auth_configured = bool(health.get("proxy_auth_configured"))
if mode != health_mode:
    emit(error_code="bridge.auth_config_mismatch", health_status=health_status,
         models_status=None, models_count=0, models_valid=False,
         identity_current=identity_current, runtime_identity=identity)
    raise SystemExit(0)
if health_mode == "required" and (not health_auth_configured or not token):
    emit(error_code="bridge.auth_secret_unavailable", health_status=health_status,
         models_status=None, models_count=0, models_valid=False,
         identity_current=identity_current, runtime_identity=identity)
    raise SystemExit(0)

models_path = "/v1/models"
if health_mode == "required":
    models_path = "/" + quote(token, safe="") + models_path

try:
    models_status, models_body = request(models_path, 1.5)
except (TimeoutError, socket.timeout):
    emit(error_code="bridge.models_timeout", health_status=health_status,
         models_status=None, models_count=0, models_valid=False,
         identity_current=identity_current, runtime_identity=identity)
    raise SystemExit(0)
except Exception:
    emit(error_code="bridge.models_unreachable", health_status=health_status,
         models_status=None, models_count=0, models_valid=False,
         identity_current=identity_current, runtime_identity=identity)
    raise SystemExit(0)

if models_status == 403:
    code = "bridge.models_auth_failed"
elif models_status != 200:
    code = "bridge.models_http"
else:
    code = "bridge.ok"

models_valid = False
models_count = 0
if models_status == 200:
    try:
        models = json.loads(models_body.decode("utf-8"))
        data = models.get("data") if isinstance(models, dict) else None
        models_valid = isinstance(data, list)
        models_count = len(data) if models_valid else 0
        if not models_valid:
            code = "bridge.models_invalid"
    except Exception:
        code = "bridge.models_invalid"

emit(error_code=code, health_status=health_status,
     models_status=models_status, models_count=models_count,
     models_valid=models_valid, identity_current=identity_current,
     runtime_identity=identity)
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SmokeCheck {
    Allow,
    Paint,
    Open,
    Bridge,
    Egress,
    Grade,
}

impl SmokeCheck {
    const ORDER: [Self; 6] = [
        Self::Allow,
        Self::Paint,
        Self::Open,
        Self::Bridge,
        Self::Egress,
        Self::Grade,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Paint => "paint",
            Self::Open => "open",
            Self::Bridge => "bridge",
            Self::Egress => "egress",
            Self::Grade => "grade",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|item| item.name() == value)
    }
}

#[derive(Debug, Clone)]
struct SmokeOptions {
    checks: Vec<SmokeCheck>,
}

#[derive(Debug, Clone, Deserialize)]
struct AllowProbePayload {
    linux_user: String,
    runtime_present: bool,
    claude_running: bool,
    claude_pid: Option<u32>,
    listener_present: bool,
    pid_8765: Option<u32>,
    pid_8766: Option<u32>,
    daemon_state: String,
    listener_probe_ok: bool,
    control_socket_present: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct AllowStatus {
    pub(crate) distro: String,
    pub(crate) linux_user: String,
    pub(crate) runtime_present: bool,
    pub(crate) claude_running: bool,
    pub(crate) claude_pid: Option<u32>,
    pub(crate) listener_present: bool,
    pub(crate) pid_8765: Option<u32>,
    pub(crate) pid_8766: Option<u32>,
    pub(crate) daemon_state: String,
    pub(crate) listener_probe_ok: bool,
    pub(crate) control_socket_present: bool,
    pub(crate) windows_bridge_pid: Option<u32>,
    pub(crate) windows_bridge_probe: String,
    pub(crate) can_open: bool,
    pub(crate) can_start: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct BridgeProbePayload {
    error_code: String,
    health_status: Option<u16>,
    models_status: Option<u16>,
    models_count: usize,
    models_valid: bool,
    identity_current: bool,
    runtime_identity: Option<Value>,
    secrets_included: bool,
}

#[derive(Debug, Clone)]
struct BridgeProbe {
    ok: bool,
    error_code: String,
    health_status: Option<u16>,
    models_status: Option<u16>,
    models_count: usize,
}

#[derive(Debug, Clone)]
struct GradeProbe {
    state: String,
    warnings: usize,
}

trait SmokeBackend {
    fn allow_status(&self) -> Result<AllowStatus, String>;
    fn resolve_login_url(&self) -> Result<String, String>;
    fn probe_bridge(&self) -> Result<BridgeProbe, String>;
    fn probe_egress(&self) -> Result<(), String>;
    fn grade_status(&self) -> GradeProbe;
}

struct LiveSmokeBackend;

impl SmokeBackend for LiveSmokeBackend {
    fn allow_status(&self) -> Result<AllowStatus, String> {
        allow_status_impl()
    }

    fn resolve_login_url(&self) -> Result<String, String> {
        get_claude_url_impl()
    }

    fn probe_bridge(&self) -> Result<BridgeProbe, String> {
        bridge_probe_impl()
    }

    fn probe_egress(&self) -> Result<(), String> {
        Err("work.bridge_egress.not_implemented".into())
    }

    fn grade_status(&self) -> GradeProbe {
        let status = current_status();
        GradeProbe {
            state: status.state,
            warnings: status.warnings.len(),
        }
    }
}

fn parse_smoke_args<I>(args: I) -> Result<Option<SmokeOptions>, String>
where
    I: IntoIterator<Item = OsString>,
{
    let args = args
        .into_iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if args.first().map(String::as_str) != Some("--smoke") {
        return Ok(None);
    }
    if args.len() == 1 {
        return Ok(Some(SmokeOptions {
            checks: SmokeCheck::ORDER.to_vec(),
        }));
    }
    if args.len() != 3 || args[1] != "--only" || args[2].trim().is_empty() {
        return Err("work.smoke.usage".into());
    }
    let mut requested = HashSet::new();
    for value in args[2].split(',').map(str::trim) {
        let Some(check) = SmokeCheck::parse(value) else {
            return Err("work.smoke.usage".into());
        };
        requested.insert(check);
    }
    let checks = SmokeCheck::ORDER
        .into_iter()
        .filter(|item| requested.contains(item))
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return Err("work.smoke.usage".into());
    }
    Ok(Some(SmokeOptions { checks }))
}

fn run_wsl_stdin(
    distro: &str,
    program: &str,
    program_args: &[&str],
    input: &str,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(distro)
        .arg("--")
        .arg(program)
        .args(program_args);
    command_output_with_stdin_timeout(command, input.as_bytes(), timeout, label)
}

fn windows_listener_pids_from_netstat(text: &str, port: u16) -> Vec<u32> {
    let suffix = format!(":{port}");
    let mut pids = text
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 5
                || !fields[0].eq_ignore_ascii_case("TCP")
                || !fields[1].ends_with(&suffix)
                || !fields[3].eq_ignore_ascii_case("LISTENING")
            {
                return None;
            }
            fields.last()?.parse::<u32>().ok().filter(|pid| *pid > 4)
        })
        .collect::<Vec<_>>();
    pids.sort_unstable();
    pids.dedup();
    pids
}

fn quick_preferred_distro() -> Result<String, String> {
    let mut command = background_command("wsl.exe");
    command.args(["--list", "--quiet"]);
    let output = command_output_with_timeout(
        command,
        DISTRO_DISCOVERY_TIMEOUT,
        "ALLOW WSL distro discovery",
    )
    .map_err(|_| "transport.wsl_distro_probe".to_string())?;
    if !output.status.success() {
        return Err("transport.wsl_distro_probe".into());
    }
    let distros = output_text(&output)
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| !value.to_ascii_lowercase().starts_with("docker-desktop"))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    preferred_distro(&distros).ok_or_else(|| "runtime.wsl_distro_missing".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsBridgeProbe {
    Absent,
    Present(u32),
    PortConflict,
    Unknown,
}

impl WindowsBridgeProbe {
    fn pid(self) -> Option<u32> {
        match self {
            Self::Present(pid) => Some(pid),
            Self::Absent | Self::PortConflict | Self::Unknown => None,
        }
    }

    fn state(self) -> &'static str {
        match self {
            Self::Absent => "checked",
            Self::Present(_) => "present",
            Self::PortConflict => "port_conflict",
            Self::Unknown => "unknown",
        }
    }

    fn safe_to_start(self) -> bool {
        matches!(self, Self::Absent)
    }
}

fn quick_windows_bridge_pid() -> WindowsBridgeProbe {
    let mut netstat = background_command("netstat.exe");
    netstat.args(["-ano", "-p", "TCP"]);
    let output = match command_output_with_timeout(
        netstat,
        WINDOWS_PORT_PROBE_TIMEOUT,
        "ALLOW Windows port",
    ) {
        Ok(output) if output.status.success() => output,
        _ => return WindowsBridgeProbe::Unknown,
    };
    let pids = windows_listener_pids_from_netstat(&output_text(&output), 9876);
    if pids.is_empty() {
        return WindowsBridgeProbe::Absent;
    }
    let pid_list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        r#"
$ErrorActionPreference='SilentlyContinue'
$ids=@({pid_list})
$verifiedId=$null
$client=[System.Net.Http.HttpClient]::new()
$client.Timeout=[TimeSpan]::FromMilliseconds(450)
try {{
  foreach($id in $ids) {{
    $process=Get-CimInstance Win32_Process -Filter "ProcessId=$id" -ErrorAction SilentlyContinue
    if(-not $process -or $process.CommandLine -notmatch '(?i)python(?:3)?(?:\.exe)?[^\r\n]*proxy\.py') {{ continue }}
    try {{
      $body=$client.GetStringAsync('http://127.0.0.1:9876/health').GetAwaiter().GetResult()
      $health=$body | ConvertFrom-Json
      $identity=$health.runtime_identity
      $managed=($identity -and $identity.managed -eq $true -and $identity.component -eq 'bridge')
      $legacy=($health.status -eq 'ok' -and [string]$health.proxy_dir -and [string]$health.source_path -match '(?i)proxy\.py$' -and $null -ne $health.PSObject.Properties['default_backend'])
      if($managed -or $legacy) {{ $verifiedId=$id; break }}
    }} catch {{}}
  }}
}} finally {{
  $client.Dispose()
}}
if($null -ne $verifiedId) {{ "CSA:$verifiedId" }} else {{ 'PORT_CONFLICT' }}
"#
    );
    let mut command = background_command("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    match command_output_with_timeout(
        command,
        WINDOWS_PROCESS_PROBE_TIMEOUT,
        "ALLOW Windows process",
    ) {
        Ok(output) if output.status.success() => {
            let result = output_text(&output);
            result
                .lines()
                .find_map(|line| line.trim().strip_prefix("CSA:"))
                .and_then(|value| value.parse::<u32>().ok())
                .map(WindowsBridgeProbe::Present)
                .unwrap_or(WindowsBridgeProbe::PortConflict)
        }
        _ => WindowsBridgeProbe::Unknown,
    }
}

fn derive_can_open(claude_running: bool, windows_bridge_pid: Option<u32>) -> bool {
    claude_running && windows_bridge_pid.is_none()
}

fn derive_can_start(
    runtime_present: bool,
    listener_probe_ok: bool,
    listener_present: bool,
    windows_probe: WindowsBridgeProbe,
) -> bool {
    runtime_present && listener_probe_ok && !listener_present && windows_probe.safe_to_start()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedBridgePackage {
    version: String,
    runtime_id: String,
    source_sha256: String,
}

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

fn expected_bridge_package_from_root(
    root: &Path,
    version: &str,
) -> Result<ExpectedBridgePackage, String> {
    const FILES: [&str; 4] = [
        "proxy.py",
        "setup-token.py",
        "requirements.txt",
        "static/dashboard.html",
    ];
    let mut manifest = String::new();
    let mut source_sha256 = None;
    for relative in FILES {
        let bytes = fs::read(root.join(relative))
            .map_err(|_| "bridge.package_source_unavailable".to_string())?;
        let file_sha = sha256_hex(bytes);
        if relative == "proxy.py" {
            source_sha256 = Some(file_sha.clone());
        }
        manifest.push_str(&format!("{file_sha}  {relative}\n"));
    }
    let bundle_sha256 = sha256_hex(manifest.as_bytes());
    Ok(ExpectedBridgePackage {
        version: version.to_string(),
        runtime_id: format!("bridge-{version}-{}", &bundle_sha256[..16]),
        source_sha256: source_sha256
            .ok_or_else(|| "bridge.package_source_unavailable".to_string())?,
    })
}

fn expected_bridge_package() -> Result<ExpectedBridgePackage, String> {
    let root = project_root().map_err(|_| "bridge.package_source_unavailable".to_string())?;
    expected_bridge_package_from_root(&root, env!("CARGO_PKG_VERSION"))
}

fn identity_matches_expected_package(
    version: &str,
    runtime_id: &str,
    source_sha256: &str,
    expected: &ExpectedBridgePackage,
) -> bool {
    version == expected.version
        && runtime_id == expected.runtime_id
        && source_sha256.eq_ignore_ascii_case(&expected.source_sha256)
}

pub(crate) fn allow_status_impl() -> Result<AllowStatus, String> {
    let windows_probe = thread::spawn(quick_windows_bridge_pid);
    let distro = quick_preferred_distro()?;
    let output = run_wsl_stdin(
        &distro,
        "bash",
        &["-s"],
        ALLOW_PROBE_SHELL,
        ALLOW_PROBE_TIMEOUT,
        "ALLOW WSL probe",
    )
    .map_err(|_| "transport.wsl_allow_probe".to_string())?;
    let windows_probe = windows_probe.join().unwrap_or(WindowsBridgeProbe::Unknown);
    let windows_bridge_pid = windows_probe.pid();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(if stderr.contains("CSA_ALLOW_DEFAULT_USER_INVALID") {
            "runtime.default_user_invalid"
        } else if stderr.contains("CSA_ALLOW_SS_MISSING") || stderr.contains("CSA_ALLOW_SS_FAILED")
        {
            "transport.listener_probe_unavailable"
        } else {
            "runtime.allow_probe_failed"
        }
        .into());
    }
    let payload: AllowProbePayload = serde_json::from_str(&output_text(&output))
        .map_err(|_| "runtime.allow_probe_invalid".to_string())?;
    let can_open = derive_can_open(payload.claude_running, windows_bridge_pid);
    let can_start = derive_can_start(
        payload.runtime_present,
        payload.listener_probe_ok,
        payload.listener_present,
        windows_probe,
    );
    Ok(AllowStatus {
        distro,
        linux_user: payload.linux_user,
        runtime_present: payload.runtime_present,
        claude_running: payload.claude_running,
        claude_pid: payload.claude_pid,
        listener_present: payload.listener_present,
        pid_8765: payload.pid_8765,
        pid_8766: payload.pid_8766,
        daemon_state: payload.daemon_state,
        listener_probe_ok: payload.listener_probe_ok,
        control_socket_present: payload.control_socket_present,
        windows_bridge_pid,
        windows_bridge_probe: windows_probe.state().into(),
        can_open,
        can_start,
    })
}

fn bridge_probe_impl() -> Result<BridgeProbe, String> {
    let expected_package = expected_bridge_package()?;
    let distro = quick_preferred_distro()?;
    let output = run_wsl_stdin(
        &distro,
        "python3",
        &["-"],
        BRIDGE_PROBE_PYTHON,
        BRIDGE_PROBE_TIMEOUT,
        "Bridge smoke probe",
    )
    .map_err(|_| "transport.wsl_bridge_probe".to_string())?;
    if !output.status.success() {
        return Err("bridge.probe_failed".into());
    }
    let payload: BridgeProbePayload = serde_json::from_str(&output_text(&output))
        .map_err(|_| "bridge.probe_invalid".to_string())?;
    if payload.secrets_included {
        return Err("bridge.probe_secret_contract".into());
    }
    let identity = payload
        .runtime_identity
        .as_ref()
        .map(|identity| serde_json::json!({ "runtime_identity": identity }))
        .as_ref()
        .and_then(|health| runtime_identity_from_health(health).ok());
    let identity_valid = identity.is_some();
    let identity_matches_package = identity
        .as_ref()
        .map(|identity| {
            identity_matches_expected_package(
                &identity.version,
                &identity.runtime_id,
                &identity.source_sha256,
                &expected_package,
            )
        })
        .unwrap_or(false);
    let ok = payload.health_status == Some(200)
        && payload.models_status == Some(200)
        && payload.models_valid
        && payload.identity_current
        && identity_valid
        && identity_matches_package;
    let error_code = bridge_error_code(&payload, identity_valid, identity_matches_package, ok);
    Ok(BridgeProbe {
        ok,
        error_code,
        health_status: payload.health_status,
        models_status: payload.models_status,
        models_count: payload.models_count,
    })
}

fn bridge_error_code(
    payload: &BridgeProbePayload,
    identity_valid: bool,
    identity_matches_package: bool,
    ok: bool,
) -> String {
    if ok {
        "bridge.ok".to_string()
    } else if payload.error_code != "bridge.ok" {
        payload.error_code.clone()
    } else if !identity_valid {
        "bridge.identity_invalid".to_string()
    } else if !payload.identity_current {
        "bridge.runtime_pointer_mismatch".to_string()
    } else if !identity_matches_package {
        "bridge.package_mismatch".to_string()
    } else {
        "bridge.contract_failed".to_string()
    }
}

fn elapsed_ms(started: Instant) -> u128 {
    started.elapsed().as_millis()
}

fn under_budget(elapsed: Duration, budget: Duration) -> bool {
    elapsed < budget
}

fn login_error_code(error: &str) -> &'static str {
    if error.contains("login.open_timeout") {
        "login.open_timeout"
    } else if error.contains("runtime.lock_held") {
        "runtime.lock_held"
    } else if error.contains("transport.") {
        "transport.claude_url_failed"
    } else if error.contains("运行时入口缺失") {
        "runtime.entry_missing"
    } else if error.contains("生命周期") || error.contains("启动或重启") {
        "login.lifecycle_busy"
    } else if error.contains("身份校验") || error.contains("未知本地服务") {
        "login.identity_unverified"
    } else if error.contains("不可中断") {
        "login.io_blocked"
    } else if error.contains("控制通道") || error.contains("control socket") {
        "login.control_unavailable"
    } else {
        "login.url_failed"
    }
}

fn run_smoke_with_backend<B: SmokeBackend>(
    options: &SmokeOptions,
    backend: &B,
    process_started: Instant,
) -> (i32, Vec<String>) {
    let total_started = Instant::now();
    let mut lines = Vec::new();
    let mut required_pass = 0usize;
    let mut required_fail = 0usize;
    let mut non_gating_fail = 0usize;
    let needs_allow =
        options.checks.contains(&SmokeCheck::Allow) || options.checks.contains(&SmokeCheck::Paint);
    let allow_result = needs_allow.then(|| {
        let result = backend.allow_status();
        let ready_elapsed = process_started.elapsed();
        (result, ready_elapsed)
    });

    for check in &options.checks {
        match check {
            SmokeCheck::Allow => match &allow_result.as_ref().expect("ALLOW result missing").0 {
                Ok(status) => {
                    required_pass += 1;
                    lines.push(format!(
                        "PASS allow {}ms inputs={} distro={} linuxUser={} claudeRunning={} claudePid={} windowsBridgePid={} windowsBridgeProbe={} runtimePresent={} listenerProbeOk={} listenerPresent={} daemonState={} pid8765={} pid8766={} controlSocket={} canOpen={} canStart={}",
                        allow_result.as_ref().expect("ALLOW result missing").1.as_millis(),
                        ALLOW_OPEN_INPUTS.join(","),
                        status.distro,
                        status.linux_user,
                        status.claude_running,
                        status.claude_pid.map(|value| value.to_string()).unwrap_or_else(|| "none".into()),
                        status.windows_bridge_pid.map(|value| value.to_string()).unwrap_or_else(|| "none".into()),
                        status.windows_bridge_probe,
                        status.runtime_present,
                        status.listener_probe_ok,
                        status.listener_present,
                        status.daemon_state,
                        status.pid_8765.map(|value| value.to_string()).unwrap_or_else(|| "none".into()),
                        status.pid_8766.map(|value| value.to_string()).unwrap_or_else(|| "none".into()),
                        status.control_socket_present,
                        status.can_open,
                        status.can_start,
                    ));
                }
                Err(code) => {
                    required_fail += 1;
                    lines.push(format!(
                        "FAIL allow {}ms {code}",
                        allow_result
                            .as_ref()
                            .expect("ALLOW result missing")
                            .1
                            .as_millis()
                    ));
                }
            },
            SmokeCheck::Paint => {
                let (result, elapsed) = allow_result.as_ref().expect("ALLOW result missing");
                if result.is_ok() && under_budget(*elapsed, PAINT_BUDGET) {
                    required_pass += 1;
                    lines.push(format!(
                        "PASS paint {}ms budget={}ms",
                        elapsed.as_millis(),
                        PAINT_BUDGET.as_millis()
                    ));
                } else {
                    required_fail += 1;
                    let code = if *elapsed >= PAINT_BUDGET {
                        "runtime.paint_timeout"
                    } else {
                        "runtime.allow_unavailable"
                    };
                    lines.push(format!("FAIL paint {}ms {code}", elapsed.as_millis()));
                }
            }
            SmokeCheck::Open => {
                let started = Instant::now();
                let result = backend.resolve_login_url();
                let elapsed = started.elapsed();
                if result.is_ok() && under_budget(elapsed, OPEN_BUDGET) {
                    required_pass += 1;
                    lines.push(format!(
                        "PASS open {}ms login.url_ready loopback=true port=8765 nonce=present",
                        elapsed.as_millis()
                    ));
                } else {
                    required_fail += 1;
                    let code = if elapsed >= OPEN_BUDGET {
                        "login.open_timeout"
                    } else {
                        result
                            .as_ref()
                            .err()
                            .map(|error| login_error_code(error))
                            .unwrap_or("login.url_failed")
                    };
                    lines.push(format!("FAIL open {}ms {code}", elapsed.as_millis()));
                }
            }
            SmokeCheck::Bridge => {
                let started = Instant::now();
                match backend.probe_bridge() {
                    Ok(report) if report.ok => {
                        required_pass += 1;
                        lines.push(format!(
                            "PASS bridge {}ms health={} models={} identity=current modelCount={}",
                            elapsed_ms(started),
                            report.health_status.unwrap_or_default(),
                            report.models_status.unwrap_or_default(),
                            report.models_count
                        ));
                    }
                    Ok(report) => {
                        required_fail += 1;
                        lines.push(format!(
                            "FAIL bridge {}ms {} health={} models={}",
                            elapsed_ms(started),
                            report.error_code,
                            report
                                .health_status
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "none".into()),
                            report
                                .models_status
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "none".into())
                        ));
                    }
                    Err(code) => {
                        required_fail += 1;
                        lines.push(format!("FAIL bridge {}ms {code}", elapsed_ms(started)));
                    }
                }
            }
            SmokeCheck::Egress => {
                let started = Instant::now();
                match backend.probe_egress() {
                    Ok(()) => lines.push(format!(
                        "PASS egress {}ms work.bridge_egress.ok",
                        elapsed_ms(started)
                    )),
                    Err(code) => {
                        non_gating_fail += 1;
                        lines.push(format!("FAIL egress {}ms {code}", elapsed_ms(started)));
                    }
                }
            }
            SmokeCheck::Grade => {
                let started = Instant::now();
                let grade = backend.grade_status();
                let level = if grade.warnings == 0 { "PASS" } else { "WARN" };
                lines.push(format!(
                    "{level} grade {}ms state={} warnings={} gating=false",
                    elapsed_ms(started),
                    grade.state,
                    grade.warnings
                ));
            }
        }
    }

    let exit_code = if required_fail == 0 { 0 } else { 1 };
    lines.push(format!(
        "SUMMARY required_pass={required_pass} required_fail={required_fail} non_gating_fail={non_gating_fail} elapsed_ms={} exit={exit_code}",
        elapsed_ms(total_started)
    ));
    (exit_code, lines)
}

pub fn smoke_exit_code_if_requested<I>(args: I, process_started: Instant) -> Option<i32>
where
    I: IntoIterator<Item = OsString>,
{
    let options = match parse_smoke_args(args) {
        Ok(None) => return None,
        Ok(Some(options)) => options,
        Err(code) => {
            eprintln!("FAIL smoke 0ms {code}");
            eprintln!("USAGE claude-science-assistant.exe --smoke [--only allow,paint,open,bridge,egress,grade]");
            return Some(2);
        }
    };
    let (exit_code, lines) = run_smoke_with_backend(&options, &LiveSmokeBackend, process_started);
    for line in lines {
        println!("{line}");
    }
    Some(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FakeBackend {
        allow_calls: Cell<usize>,
        bridge_ok: bool,
        grade_warnings: usize,
    }

    impl SmokeBackend for FakeBackend {
        fn allow_status(&self) -> Result<AllowStatus, String> {
            self.allow_calls.set(self.allow_calls.get() + 1);
            Ok(AllowStatus {
                distro: "Ubuntu-24.04".into(),
                linux_user: "test-user".into(),
                runtime_present: true,
                claude_running: true,
                claude_pid: Some(42),
                listener_present: true,
                pid_8765: Some(42),
                pid_8766: Some(42),
                daemon_state: "managed_ready".into(),
                listener_probe_ok: true,
                control_socket_present: true,
                windows_bridge_pid: None,
                windows_bridge_probe: "checked".into(),
                can_open: true,
                can_start: false,
            })
        }

        fn resolve_login_url(&self) -> Result<String, String> {
            Ok("http://127.0.0.1:8765/?nonce=must-never-appear".into())
        }

        fn probe_bridge(&self) -> Result<BridgeProbe, String> {
            Ok(BridgeProbe {
                ok: self.bridge_ok,
                error_code: if self.bridge_ok {
                    "bridge.ok".into()
                } else {
                    "bridge.revision_mismatch".into()
                },
                health_status: Some(200),
                models_status: Some(200),
                models_count: 3,
            })
        }

        fn probe_egress(&self) -> Result<(), String> {
            Err("work.bridge_egress.proxy_dead".into())
        }

        fn grade_status(&self) -> GradeProbe {
            GradeProbe {
                state: "degraded".into(),
                warnings: self.grade_warnings,
            }
        }
    }

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn smoke_argument_parser_is_small_and_deterministic() {
        assert!(parse_smoke_args(args(&[])).unwrap().is_none());
        assert!(parse_smoke_args(args(&["--not-smoke"])).unwrap().is_none());
        let all = parse_smoke_args(args(&["--smoke"])).unwrap().unwrap();
        assert_eq!(all.checks, SmokeCheck::ORDER);
        let selected = parse_smoke_args(args(&["--smoke", "--only", "bridge,allow,bridge,open"]))
            .unwrap()
            .unwrap();
        assert_eq!(
            selected.checks,
            vec![SmokeCheck::Allow, SmokeCheck::Open, SmokeCheck::Bridge]
        );
        assert!(parse_smoke_args(args(&["--smoke", "--only"])).is_err());
        assert!(parse_smoke_args(args(&["--smoke", "--only", "unknown"])).is_err());
    }

    #[test]
    fn smoke_reuses_allow_snapshot_and_work_grade_cannot_fail_the_gate() {
        let backend = FakeBackend {
            allow_calls: Cell::new(0),
            bridge_ok: true,
            grade_warnings: 7,
        };
        let options = SmokeOptions {
            checks: SmokeCheck::ORDER.to_vec(),
        };
        let (exit, lines) = run_smoke_with_backend(&options, &backend, Instant::now());
        assert_eq!(exit, 0);
        assert_eq!(backend.allow_calls.get(), 1);
        let output = lines.join("\n");
        assert!(output.contains("FAIL egress"));
        assert!(output.contains("WARN grade"));
        assert!(output.contains("exit=0"));
        assert!(!output.contains("must-never-appear"));
        assert!(output.contains("nonce=present"));
    }

    #[test]
    fn bridge_failure_is_required_and_budget_boundaries_are_strict() {
        let backend = FakeBackend {
            allow_calls: Cell::new(0),
            bridge_ok: false,
            grade_warnings: 0,
        };
        let options = SmokeOptions {
            checks: vec![SmokeCheck::Bridge, SmokeCheck::Egress, SmokeCheck::Grade],
        };
        let (exit, lines) = run_smoke_with_backend(&options, &backend, Instant::now());
        assert_eq!(exit, 1);
        assert!(lines.join("\n").contains("bridge.revision_mismatch"));
        assert!(under_budget(Duration::from_millis(2_999), PAINT_BUDGET));
        assert!(!under_budget(Duration::from_millis(3_000), PAINT_BUDGET));
        assert!(under_budget(Duration::from_millis(7_999), OPEN_BUDGET));
        assert!(!under_budget(Duration::from_millis(8_000), OPEN_BUDGET));

        let unreachable = BridgeProbePayload {
            error_code: "bridge.health_unreachable".into(),
            health_status: None,
            models_status: None,
            models_count: 0,
            models_valid: false,
            identity_current: false,
            runtime_identity: None,
            secrets_included: false,
        };
        assert_eq!(
            bridge_error_code(&unreachable, false, false, false),
            "bridge.health_unreachable"
        );

        let invalid_health = BridgeProbePayload {
            error_code: "bridge.health_invalid".into(),
            health_status: Some(200),
            models_status: None,
            models_count: 0,
            models_valid: false,
            identity_current: false,
            runtime_identity: None,
            secrets_included: false,
        };
        assert_eq!(
            bridge_error_code(&invalid_health, false, false, false),
            "bridge.health_invalid"
        );

        let expected = ExpectedBridgePackage {
            version: env!("CARGO_PKG_VERSION").into(),
            runtime_id: "bridge-0.1.6-0123456789abcdef".into(),
            source_sha256: "a".repeat(64),
        };
        assert!(identity_matches_expected_package(
            env!("CARGO_PKG_VERSION"),
            "bridge-0.1.6-0123456789abcdef",
            &"a".repeat(64),
            &expected,
        ));
        assert!(!identity_matches_expected_package(
            env!("CARGO_PKG_VERSION"),
            "bridge-0.1.6-0123456789abcdef",
            &"b".repeat(64),
            &expected,
        ));
    }

    #[test]
    fn expected_bridge_package_hash_contract_matches_runtime_layout() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "csa-smoke-bridge-fixture-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("static")).unwrap();
        fs::write(root.join("proxy.py"), b"proxy\n").unwrap();
        fs::write(root.join("setup-token.py"), b"token\n").unwrap();
        fs::write(root.join("requirements.txt"), b"requirements\n").unwrap();
        fs::write(root.join("static/dashboard.html"), b"<html/>\n").unwrap();

        let initial = expected_bridge_package_from_root(&root, "0.1.6").unwrap();
        assert_eq!(initial.source_sha256, sha256_hex(b"proxy\n"));
        assert_eq!(initial.runtime_id, "bridge-0.1.6-582fa1d5c5e51cae");

        fs::write(
            root.join("static/dashboard.html"),
            b"<html>changed</html>\n",
        )
        .unwrap();
        let dashboard_changed = expected_bridge_package_from_root(&root, "0.1.6").unwrap();
        assert_eq!(dashboard_changed.source_sha256, initial.source_sha256);
        assert_ne!(dashboard_changed.runtime_id, initial.runtime_id);

        fs::write(root.join("proxy.py"), b"proxy changed\n").unwrap();
        let proxy_changed = expected_bridge_package_from_root(&root, "0.1.6").unwrap();
        assert_ne!(proxy_changed.source_sha256, initial.source_sha256);
        assert_ne!(proxy_changed.runtime_id, initial.runtime_id);

        fs::remove_file(root.join("requirements.txt")).unwrap();
        assert_eq!(
            expected_bridge_package_from_root(&root, "0.1.6").unwrap_err(),
            "bridge.package_source_unavailable"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn allow_transport_is_stdin_bash_and_has_no_grade_calls() {
        let source = include_str!("smoke.rs");
        let script_start = source.find("const ALLOW_PROBE_SHELL").unwrap();
        let script_end = source[script_start..]
            .find("const BRIDGE_PROBE_PYTHON")
            .unwrap()
            + script_start;
        let helper_start = source
            .find("fn windows_listener_pids_from_netstat")
            .unwrap();
        let helper_end =
            source[helper_start..].find("fn bridge_probe_impl").unwrap() + helper_start;
        let allow = format!(
            "{}\n{}",
            &source[script_start..script_end],
            &source[helper_start..helper_end]
        );
        assert!(allow.contains("ALLOW_PROBE_SHELL"));
        assert!(allow.contains("&[\"-s\"]"));
        assert!(!allow.contains("bash -lc"));
        for forbidden in [
            "current_status",
            "inspect_wsl_runtime",
            "windows_storage_snapshot",
            "csa-network-quality",
            "vhdx",
            "canary",
            "mcp",
        ] {
            assert!(
                !allow.to_ascii_lowercase().contains(forbidden),
                "ALLOW fast path gained forbidden GRADE/WORK input: {forbidden}"
            );
        }
        assert_eq!(ALLOW_OPEN_INPUTS, ["claudeRunning", "windowsBridgePid"]);
        assert!(derive_can_open(true, WindowsBridgeProbe::Unknown.pid()));
        assert!(derive_can_open(
            true,
            WindowsBridgeProbe::PortConflict.pid()
        ));
        assert!(!derive_can_open(
            true,
            WindowsBridgeProbe::Present(41).pid()
        ));
        assert!(!WindowsBridgeProbe::Unknown.safe_to_start());
        assert!(!WindowsBridgeProbe::PortConflict.safe_to_start());
        assert!(WindowsBridgeProbe::Absent.safe_to_start());
        assert!(derive_can_start(
            true,
            true,
            false,
            WindowsBridgeProbe::Absent
        ));
        assert!(!derive_can_start(
            true,
            false,
            false,
            WindowsBridgeProbe::Absent
        ));
        assert!(!derive_can_start(
            true,
            true,
            true,
            WindowsBridgeProbe::Absent
        ));
        assert!(!derive_can_start(
            true,
            true,
            false,
            WindowsBridgeProbe::Unknown
        ));
        assert!(DISTRO_DISCOVERY_TIMEOUT + ALLOW_PROBE_TIMEOUT < PAINT_BUDGET);
        assert!(WINDOWS_PORT_PROBE_TIMEOUT + WINDOWS_PROCESS_PROBE_TIMEOUT < PAINT_BUDGET);
        assert!(allow.contains("id -un"));
        assert!(!allow.contains("selected_linux_user_quick"));
        assert!(allow.contains("listeners_8765"));
        assert!(allow.contains("if [ -n \"$listeners_8765\" ]"));
        let runtime_presence_start = allow.find("runtime_present=false").unwrap();
        let runtime_presence_end =
            allow[runtime_presence_start..].find("pids_8765=").unwrap() + runtime_presence_start;
        let runtime_presence = &allow[runtime_presence_start..runtime_presence_end];
        assert!(runtime_presence.contains("runtime/claude-science/versions/*"));
        assert!(!runtime_presence.contains("patched-current"));
        assert!(!runtime_presence.contains(".local/bin"));
        assert!(!runtime_presence.contains("legacy_root"));
        assert_eq!(
            windows_listener_pids_from_netstat(
                "  TCP    127.0.0.1:9876    0.0.0.0:0    LISTENING    4321\r\n",
                9876,
            ),
            vec![4321]
        );
        assert!(windows_listener_pids_from_netstat(
            "  TCP    127.0.0.1:8765    0.0.0.0:0    LISTENING    4321\r\n",
            9876,
        )
        .is_empty());
    }
}
