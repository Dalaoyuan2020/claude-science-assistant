use super::bridge_egress::{
    consume_recent_candidate_proof, run_bridge_egress_probe, BridgeEgressLayerState,
    BridgeEgressReport,
};
use super::{
    background_command, bridge_config_transition_lock, clean_diagnostic_text, command_error_text,
    command_output_with_stdin_timeout, discover_distros_with_timeout, ensure_error_prefix,
    output_text, preferred_distro, run_blocking, service_operation_lock,
};
use serde::Serialize;
use std::time::Duration;

// The host watchdog must outlive the guest `timeout` so WSL can return the
// script's own diagnostic. POST/identity checks can legitimately consume
// roughly ten seconds on a loaded WSL instance; rollback gets extra headroom
// because it is the recovery path and performs another identity revalidation.
const APPLY_COMMAND_BUDGET: Duration = Duration::from_secs(18);
const ROLLBACK_COMMAND_BUDGET: Duration = Duration::from_secs(24);
const APPLY_DISTRO_DISCOVERY_BUDGET: Duration = Duration::from_millis(1_500);

const BACKUP_CONFIG_SCRIPT: &str = r#"
import datetime
import hashlib
import http.client
import json
import os
import pathlib
import re
import stat
import subprocess
import sys
import time
from urllib.parse import urlsplit


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


try:
    connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=2.0)
    connection.request("GET", "/health", headers={"Connection": "close"})
    response = connection.getresponse()
    payload = response.read(1024 * 1024 + 1)
    connection.close()
    if response.status != 200 or len(payload) > 1024 * 1024:
        fail("managed Bridge health is unavailable")
    health = json.loads(payload.decode("utf-8"))
except Exception:
    fail("managed Bridge health is unavailable")

identity = health.get("runtime_identity") if isinstance(health, dict) else None
if not isinstance(identity, dict) or identity.get("managed") is not True:
    fail("managed Bridge identity is missing")
try:
    pid = int(identity.get("pid"))
    source_path = pathlib.Path(str(identity.get("sourcePath") or "")).resolve()
    source_sha256 = str(identity.get("sourceSha256") or "").lower()
    current_path = (
        pathlib.Path.home()
        / ".local"
        / "share"
        / "csa"
        / "runtime"
        / "bridge"
        / "current"
        / "proxy.py"
    ).resolve()
    argv = (pathlib.Path("/proc") / str(pid) / "cmdline").read_bytes().split(b"\0")
    argv_paths = []
    for item in argv:
        value = item.decode("utf-8", "ignore")
        if value.endswith("proxy.py"):
            argv_paths.append(pathlib.Path(value).resolve())
    actual_sha256 = hashlib.sha256(current_path.read_bytes()).hexdigest()
    sockets = subprocess.run(
        ["ss", "-H", "-ltnp", "sport = :9876"],
        capture_output=True, text=True, timeout=0.8, check=False,
    )
    listener_pids = set(int(value) for value in re.findall(r"pid=(\d+)", sockets.stdout))
except Exception:
    fail("managed Bridge identity cannot be verified")
if (
    pid <= 0
    or source_path != current_path
    or current_path not in argv_paths
    or len(source_sha256) != 64
    or actual_sha256.lower() != source_sha256
    or listener_pids != {pid}
):
    fail("managed Bridge identity does not match current runtime")

environment = {}
try:
    environ = (pathlib.Path("/proc") / str(pid) / "environ").read_bytes()
    for item in environ.split(b"\0"):
        if b"=" not in item:
            continue
        key, value = item.split(b"=", 1)
        environment[key.decode("utf-8", "ignore")] = value.decode("utf-8", "ignore")
except Exception:
    fail("managed Bridge environment cannot be read")

configured_root = str(environment.get("CLAUDE_SCIENCE_PROXY_DIR") or "").strip()
expected_root = (
    pathlib.Path(configured_root).expanduser()
    if configured_root
    else pathlib.Path.home() / ".claude-science" / "proxy"
).resolve()
health_root = pathlib.Path(str(health.get("proxy_dir") or "")).resolve()
if health_root != expected_root:
    fail("managed Bridge config directory does not match health")

config_path = expected_root / "config.json"
if not config_path.is_file() or config_path.stat().st_size > 4 * 1024 * 1024:
    fail("managed Bridge config is missing or too large")
try:
    config_value = json.loads(config_path.read_text(encoding="utf-8"))
except Exception:
    fail("managed Bridge config is not valid JSON")
if not isinstance(config_value, dict):
    fail("managed Bridge config is not a JSON object")

backup_path = None
for _ in range(3):
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    candidate = config_path.with_name("config.json.bak-" + stamp)
    try:
        descriptor = os.open(candidate, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        backup_path = candidate
        break
    except FileExistsError:
        time.sleep(1.0)
if backup_path is None:
    fail("a unique timestamped backup name could not be allocated")

try:
    with os.fdopen(descriptor, "wb") as destination:
        destination.write(config_path.read_bytes())
        destination.flush()
        os.fsync(destination.fileno())
    os.chmod(backup_path, 0o600)
    if stat.S_IMODE(backup_path.stat().st_mode) != 0o600:
        fail("backup permissions are not 0600")
except Exception:
    try:
        backup_path.unlink(missing_ok=True)
    except Exception:
        pass
    fail("managed Bridge config backup failed")

before = ""
raw_before = health.get("outbound_proxy_url")
if isinstance(raw_before, str) and raw_before.strip():
    try:
        parts = urlsplit(raw_before.strip())
        scheme = parts.scheme.lower()
        host = parts.hostname or ""
        if scheme not in {"http", "https", "socks5", "socks5h"} or not host:
            before = "configured value redacted"
        else:
            defaults = {"http": 80, "https": 443, "socks5": 1080, "socks5h": 1080}
            port = parts.port or defaults[scheme]
            display_host = "[" + host + "]" if ":" in host else host
            before = f"{scheme}://{display_host}:{port}"
    except Exception:
        before = "configured value redacted"
print(json.dumps({
    "configPath": str(config_path),
    "backupPath": str(backup_path),
    "beforeOutboundProxyUrl": before,
}, ensure_ascii=True, separators=(",", ":")))
"#;

const POST_CONFIG_SCRIPT: &str = r#"
import hashlib
import http.client
import json
import pathlib
import re
import subprocess
import sys


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


def managed_config_path():
    try:
        connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=2.0)
        connection.request("GET", "/health", headers={"Connection": "close"})
        response = connection.getresponse()
        payload = response.read(1024 * 1024 + 1)
        connection.close()
        if response.status != 200 or len(payload) > 1024 * 1024:
            fail("managed Bridge health is unavailable")
        health = json.loads(payload.decode("utf-8"))
        identity = health.get("runtime_identity") if isinstance(health, dict) else None
        if not isinstance(identity, dict) or identity.get("managed") is not True:
            fail("managed Bridge identity is missing")
        pid = int(identity.get("pid"))
        source_path = pathlib.Path(str(identity.get("sourcePath") or "")).resolve()
        source_sha256 = str(identity.get("sourceSha256") or "").lower()
        current_path = (
            pathlib.Path.home() / ".local" / "share" / "csa" / "runtime"
            / "bridge" / "current" / "proxy.py"
        ).resolve()
        argv = (pathlib.Path("/proc") / str(pid) / "cmdline").read_bytes().split(b"\0")
        argv_paths = []
        for item in argv:
            value = item.decode("utf-8", "ignore")
            if value.endswith("proxy.py"):
                argv_paths.append(pathlib.Path(value).resolve())
        actual_sha256 = hashlib.sha256(current_path.read_bytes()).hexdigest()
        sockets = subprocess.run(
            ["ss", "-H", "-ltnp", "sport = :9876"],
            capture_output=True, text=True, timeout=0.8, check=False,
        )
        listener_pids = set(int(value) for value in re.findall(r"pid=(\d+)", sockets.stdout))
        if (
            pid <= 0
            or source_path != current_path
            or current_path not in argv_paths
            or len(source_sha256) != 64
            or actual_sha256.lower() != source_sha256
            or listener_pids != {pid}
        ):
            fail("managed Bridge identity does not match current listener")
        environment = {}
        environ = (pathlib.Path("/proc") / str(pid) / "environ").read_bytes()
        for item in environ.split(b"\0"):
            if b"=" in item:
                key, value = item.split(b"=", 1)
                environment[key.decode("utf-8", "ignore")] = value.decode("utf-8", "ignore")
        configured_root = str(environment.get("CLAUDE_SCIENCE_PROXY_DIR") or "").strip()
        expected_root = (
            pathlib.Path(configured_root).expanduser()
            if configured_root
            else pathlib.Path.home() / ".claude-science" / "proxy"
        ).resolve()
        health_root = pathlib.Path(str(health.get("proxy_dir") or "")).resolve()
        if health_root != expected_root:
            fail("managed Bridge config directory does not match health")
        return (expected_root / "config.json").resolve()
    except SystemExit:
        raise
    except Exception:
        fail("managed Bridge current listener cannot be verified")


try:
    body = bytes.fromhex(sys.argv[1])
    config_path = pathlib.Path(bytes.fromhex(sys.argv[2]).decode("utf-8")).resolve()
    patch = json.loads(body.decode("utf-8"))
except Exception:
    fail("partial config request is invalid")
if not isinstance(patch, dict) or list(patch.keys()) != ["outbound_proxy_url"]:
    fail("partial config request contains unexpected keys")
if not isinstance(patch["outbound_proxy_url"], str):
    fail("outbound proxy value is not a string")
if config_path != managed_config_path():
    fail("partial config path is not owned by the current managed Bridge")

try:
    data = json.loads(config_path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        fail("managed Bridge config is not a JSON object")
except Exception:
    fail("managed Bridge config cannot be read")
headers = {
    "Connection": "close",
    "Content-Type": "application/json",
    "Content-Length": str(len(body)),
}
mode = str(data.get("proxy_auth_mode") or "optional").strip().lower()
token = str(data.get("proxy_auth_token") or "").strip()
if mode == "required":
    if not token:
        fail("local management authentication is required but unavailable")
    headers["X-Proxy-Control-Token"] = token

# Narrow the check-to-use window before a control token can leave this process.
if config_path != managed_config_path():
    fail("managed Bridge listener changed before partial config POST")

try:
    connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=4.0)
    connection.request("POST", "/api/config", body=body, headers=headers)
    response = connection.getresponse()
    response_body = response.read(1024 * 1024 + 1)
    connection.close()
except Exception:
    fail("local partial config POST failed")
if response.status != 200 or len(response_body) > 1024 * 1024:
    fail("local partial config POST was rejected")
try:
    result = json.loads(response_body.decode("utf-8"))
except Exception:
    fail("local partial config POST returned invalid JSON")
if not isinstance(result, dict) or result.get("ok") is not True:
    fail("local partial config POST did not apply the field")
print('{"ok":true}')
"#;

const READ_HEALTH_SCRIPT: &str = r#"
import hashlib
import http.client
import json
import pathlib
import re
import subprocess
import sys


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


try:
    connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=2.0)
    connection.request("GET", "/health", headers={"Connection": "close"})
    response = connection.getresponse()
    body = response.read(1024 * 1024 + 1)
    connection.close()
except Exception:
    fail("managed Bridge health read failed")
if response.status != 200 or len(body) > 1024 * 1024:
    fail("managed Bridge health read was rejected")
try:
    health = json.loads(body.decode("utf-8"))
except Exception:
    fail("managed Bridge health is invalid JSON")
try:
    identity = health.get("runtime_identity") if isinstance(health, dict) else None
    if not isinstance(identity, dict) or identity.get("managed") is not True:
        fail("managed Bridge identity is missing")
    pid = int(identity.get("pid"))
    current_path = (
        pathlib.Path.home() / ".local" / "share" / "csa" / "runtime"
        / "bridge" / "current" / "proxy.py"
    ).resolve()
    source_path = pathlib.Path(str(identity.get("sourcePath") or "")).resolve()
    source_sha256 = str(identity.get("sourceSha256") or "").lower()
    actual_sha256 = hashlib.sha256(current_path.read_bytes()).hexdigest()
    argv = (pathlib.Path("/proc") / str(pid) / "cmdline").read_bytes().split(b"\0")
    argv_paths = [
        pathlib.Path(item.decode("utf-8", "ignore")).resolve()
        for item in argv if item.decode("utf-8", "ignore").endswith("proxy.py")
    ]
    sockets = subprocess.run(
        ["ss", "-H", "-ltnp", "sport = :9876"],
        capture_output=True, text=True, timeout=0.8, check=False,
    )
    listener_pids = set(int(value) for value in re.findall(r"pid=(\d+)", sockets.stdout))
    if (
        pid <= 0
        or source_path != current_path
        or current_path not in argv_paths
        or len(source_sha256) != 64
        or actual_sha256.lower() != source_sha256
        or listener_pids != {pid}
    ):
        fail("managed Bridge health does not belong to the current listener")
except SystemExit:
    raise
except Exception:
    fail("managed Bridge health identity cannot be verified")
value = health.get("outbound_proxy_url") if isinstance(health, dict) else None
if not isinstance(value, str):
    fail("managed Bridge health omitted outbound proxy state")
print(json.dumps({
    "outboundProxyUrl": value,
    "configRevision": str(health.get("config_revision") or ""),
}, ensure_ascii=True, separators=(",", ":")))
"#;

const RESTORE_CONFIG_SCRIPT: &str = r#"
import hashlib
import http.client
import json
import pathlib
import re
import stat
import subprocess
import sys
from urllib.parse import urlsplit, urlunsplit


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


def mask_url_credentials(url):
    if not url:
        return ""
    try:
        parts = urlsplit(url)
        if "@" not in parts.netloc:
            return url
        host = parts.hostname or ""
        if parts.port:
            host = f"{host}:{parts.port}"
        return urlunsplit((parts.scheme, f"****@{host}", parts.path, parts.query, parts.fragment))
    except Exception:
        return "****"


def managed_context():
    try:
        connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=2.0)
        connection.request("GET", "/health", headers={"Connection": "close"})
        response = connection.getresponse()
        payload = response.read(1024 * 1024 + 1)
        connection.close()
        if response.status != 200 or len(payload) > 1024 * 1024:
            fail("managed Bridge health is unavailable during rollback")
        health = json.loads(payload.decode("utf-8"))
        identity = health.get("runtime_identity") if isinstance(health, dict) else None
        if not isinstance(identity, dict) or identity.get("managed") is not True:
            fail("managed Bridge identity is missing during rollback")
        pid = int(identity.get("pid"))
        source_path = pathlib.Path(str(identity.get("sourcePath") or "")).resolve()
        source_sha256 = str(identity.get("sourceSha256") or "").lower()
        current_path = (
            pathlib.Path.home() / ".local" / "share" / "csa" / "runtime"
            / "bridge" / "current" / "proxy.py"
        ).resolve()
        argv = (pathlib.Path("/proc") / str(pid) / "cmdline").read_bytes().split(b"\0")
        argv_paths = []
        for item in argv:
            value = item.decode("utf-8", "ignore")
            if value.endswith("proxy.py"):
                argv_paths.append(pathlib.Path(value).resolve())
        actual_sha256 = hashlib.sha256(current_path.read_bytes()).hexdigest()
        sockets = subprocess.run(
            ["ss", "-H", "-ltnp", "sport = :9876"],
            capture_output=True, text=True, timeout=0.8, check=False,
        )
        listener_pids = set(int(value) for value in re.findall(r"pid=(\d+)", sockets.stdout))
        if (
            pid <= 0
            or source_path != current_path
            or current_path not in argv_paths
            or len(source_sha256) != 64
            or actual_sha256.lower() != source_sha256
            or listener_pids != {pid}
        ):
            fail("managed Bridge identity does not match current listener during rollback")
        environment = {}
        environ = (pathlib.Path("/proc") / str(pid) / "environ").read_bytes()
        for item in environ.split(b"\0"):
            if b"=" in item:
                key, value = item.split(b"=", 1)
                environment[key.decode("utf-8", "ignore")] = value.decode("utf-8", "ignore")
        configured_root = str(environment.get("CLAUDE_SCIENCE_PROXY_DIR") or "").strip()
        expected_root = (
            pathlib.Path(configured_root).expanduser()
            if configured_root
            else pathlib.Path.home() / ".claude-science" / "proxy"
        ).resolve()
        health_root = pathlib.Path(str(health.get("proxy_dir") or "")).resolve()
        if health_root != expected_root:
            fail("managed Bridge config directory does not match health during rollback")
        return health, (expected_root / "config.json").resolve()
    except SystemExit:
        raise
    except Exception:
        fail("managed Bridge current listener cannot be verified during rollback")


try:
    config_path = pathlib.Path(bytes.fromhex(sys.argv[1]).decode("utf-8")).resolve()
    backup_path = pathlib.Path(bytes.fromhex(sys.argv[2]).decode("utf-8")).resolve()
except Exception:
    fail("rollback paths are invalid")
if (
    not config_path.is_absolute()
    or not backup_path.is_absolute()
    or config_path.name != "config.json"
    or backup_path.parent != config_path.parent
    or re.fullmatch(r"config\.json\.bak-\d{8}-\d{6}", backup_path.name) is None
    or not backup_path.is_file()
    or stat.S_IMODE(backup_path.stat().st_mode) != 0o600
):
    fail("rollback backup identity is invalid")
_, managed_path = managed_context()
if config_path != managed_path:
    fail("rollback config path is not owned by the current managed Bridge")
try:
    backup_value = json.loads(backup_path.read_text(encoding="utf-8"))
    current_value = json.loads(config_path.read_text(encoding="utf-8"))
except Exception:
    fail("rollback config cannot be read")
if not isinstance(backup_value, dict) or not isinstance(current_value, dict):
    fail("rollback config is not a JSON object")
original_proxy = backup_value.get("outbound_proxy_url", "")
if not isinstance(original_proxy, str):
    fail("rollback outbound proxy value is invalid")

# Restore only the live Bridge field through the same single-key management
# API. The 0600 backup remains evidence and a source for the old field value;
# never reinstall the whole file over unrelated concurrent changes.
body = json.dumps(
    {"outbound_proxy_url": original_proxy},
    ensure_ascii=False,
    separators=(",", ":"),
).encode("utf-8")
headers = {
    "Connection": "close",
    "Content-Type": "application/json",
    "Content-Length": str(len(body)),
}
mode = str(current_value.get("proxy_auth_mode") or "optional").strip().lower()
token = str(current_value.get("proxy_auth_token") or "").strip()
if mode == "required":
    if not token:
        fail("rollback management authentication is unavailable")
    headers["X-Proxy-Control-Token"] = token

# Do not send the management token if 9876 changed owners after it was read.
_, managed_path = managed_context()
if config_path != managed_path:
    fail("managed Bridge listener changed before rollback POST")
try:
    connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=4.0)
    connection.request("POST", "/api/config", body=body, headers=headers)
    response = connection.getresponse()
    response_body = response.read(1024 * 1024 + 1)
    connection.close()
    result = json.loads(response_body.decode("utf-8"))
    live_rollback_ok = (
        response.status == 200
        and len(response_body) <= 1024 * 1024
        and isinstance(result, dict)
        and result.get("ok") is True
    )
except Exception:
    live_rollback_ok = False
if not live_rollback_ok:
    fail("single-field live Bridge rollback failed; backup was retained for recovery")
health_after, managed_path = managed_context()
if (
    config_path != managed_path
    or health_after.get("outbound_proxy_url") != mask_url_credentials(original_proxy)
):
    fail("single-field rollback was not reflected by the managed Bridge")
print('{"ok":true}')
"#;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEgressApplyReport {
    pub operation: String,
    pub ok: bool,
    pub code: String,
    pub backup_path: String,
    pub before_outbound_proxy_url: String,
    pub after_outbound_proxy_url: String,
    pub after_probe: BridgeEgressReport,
}

#[derive(Debug, Clone)]
struct BridgeConfigBackup {
    config_path: String,
    backup_path: String,
    before_outbound_proxy_url: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupScriptResult {
    config_path: String,
    backup_path: String,
    before_outbound_proxy_url: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HealthScriptResult {
    outbound_proxy_url: String,
    #[allow(dead_code)]
    config_revision: String,
}

trait BridgeEgressApplyOps {
    fn backup_config(&mut self) -> Result<BridgeConfigBackup, String>;
    fn post_partial_config(
        &mut self,
        backup: &BridgeConfigBackup,
        body: &[u8],
    ) -> Result<(), String>;
    fn read_health_outbound_proxy_url(&mut self) -> Result<String, String>;
    fn run_egress_probe(&mut self) -> Result<BridgeEgressReport, String>;
    fn restore_backup(&mut self, backup: &BridgeConfigBackup) -> Result<(), String>;
}

struct WslBridgeEgressApplyOps {
    distro: String,
}

impl WslBridgeEgressApplyOps {
    fn run_python(
        &self,
        script: &str,
        hex_arguments: &[String],
        timeout: Duration,
        label: &str,
    ) -> Result<String, String> {
        let guest_timeout_seconds = timeout.as_secs().saturating_sub(3).max(1);
        let mut command = background_command("wsl.exe");
        command
            .arg("--distribution")
            .arg(&self.distro)
            .arg("--")
            .arg("timeout")
            .arg("--signal=TERM")
            .arg("--kill-after=1s")
            .arg(format!("{guest_timeout_seconds}s"))
            .arg("python3")
            .arg("-");
        for argument in hex_arguments {
            if argument.is_empty()
                || !argument
                    .bytes()
                    .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase())
            {
                return Err("work.bridge_egress.apply_transport_invalid".into());
            }
            command.arg(argument);
        }
        let output = command_output_with_stdin_timeout(command, script.as_bytes(), timeout, label)
            .map_err(|error| clean_diagnostic_text(&error))?;
        if !output.status.success() {
            let detail = clean_diagnostic_text(&command_error_text(&output));
            return Err(if detail.trim().is_empty() {
                format!("{label} failed")
            } else {
                detail
            });
        }
        Ok(output_text(&output))
    }
}

impl BridgeEgressApplyOps for WslBridgeEgressApplyOps {
    fn backup_config(&mut self) -> Result<BridgeConfigBackup, String> {
        let payload = self.run_python(
            BACKUP_CONFIG_SCRIPT,
            &[],
            APPLY_COMMAND_BUDGET,
            "Bridge egress config backup",
        )?;
        let result: BackupScriptResult = serde_json::from_str(&payload)
            .map_err(|_| "Bridge egress backup returned invalid metadata".to_string())?;
        if result.config_path.is_empty()
            || result.backup_path.is_empty()
            || result
                .before_outbound_proxy_url
                .chars()
                .any(char::is_control)
        {
            return Err("Bridge egress backup metadata is invalid".into());
        }
        Ok(BridgeConfigBackup {
            config_path: result.config_path,
            backup_path: result.backup_path,
            before_outbound_proxy_url: result.before_outbound_proxy_url,
        })
    }

    fn post_partial_config(
        &mut self,
        backup: &BridgeConfigBackup,
        body: &[u8],
    ) -> Result<(), String> {
        let arguments = [hex_encode(body), hex_encode(backup.config_path.as_bytes())];
        self.run_python(
            POST_CONFIG_SCRIPT,
            &arguments,
            APPLY_COMMAND_BUDGET,
            "Bridge egress partial config POST",
        )?;
        Ok(())
    }

    fn read_health_outbound_proxy_url(&mut self) -> Result<String, String> {
        let payload = self.run_python(
            READ_HEALTH_SCRIPT,
            &[],
            APPLY_COMMAND_BUDGET,
            "Bridge egress health verification",
        )?;
        let result: HealthScriptResult = serde_json::from_str(&payload)
            .map_err(|_| "Bridge egress health returned invalid metadata".to_string())?;
        normalize_candidate_url(&result.outbound_proxy_url)
            .map_err(|_| "Bridge egress health returned an unsafe proxy value".to_string())
    }

    fn run_egress_probe(&mut self) -> Result<BridgeEgressReport, String> {
        run_bridge_egress_probe(true)
    }

    fn restore_backup(&mut self, backup: &BridgeConfigBackup) -> Result<(), String> {
        let arguments = [
            hex_encode(backup.config_path.as_bytes()),
            hex_encode(backup.backup_path.as_bytes()),
        ];
        self.run_python(
            RESTORE_CONFIG_SCRIPT,
            &arguments,
            ROLLBACK_COMMAND_BUDGET,
            "Bridge egress config rollback",
        )?;
        Ok(())
    }
}

fn hex_encode(value: &[u8]) -> String {
    value
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn normalize_candidate_url(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("direct") {
        return Ok(String::new());
    }
    if value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
        || value.contains('@')
        || value.contains('?')
        || value.contains('#')
    {
        return Err("work.bridge_egress.apply_candidate_invalid".into());
    }
    let Some((raw_scheme, endpoint)) = value.split_once("://") else {
        return Err("work.bridge_egress.apply_candidate_invalid".into());
    };
    let scheme = raw_scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "socks5" | "socks5h")
        || endpoint.is_empty()
        || endpoint.contains('/')
    {
        return Err("work.bridge_egress.apply_candidate_invalid".into());
    }
    let (host, port_text, display_host) = if let Some(rest) = endpoint.strip_prefix('[') {
        let Some((host, port_text)) = rest.split_once("]:") else {
            return Err("work.bridge_egress.apply_candidate_invalid".into());
        };
        (host, port_text, format!("[{host}]"))
    } else {
        let Some((host, port_text)) = endpoint.rsplit_once(':') else {
            return Err("work.bridge_egress.apply_candidate_invalid".into());
        };
        (host, port_text, host.to_ascii_lowercase())
    };
    let host_is_valid = if host.contains(':') {
        host.parse::<std::net::Ipv6Addr>().is_ok()
    } else {
        host.len() <= 253
            && !host.is_empty()
            && host
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'.' | b'-'))
            && !host.starts_with('.')
            && !host.ends_with('.')
    };
    if !host_is_valid {
        return Err("work.bridge_egress.apply_candidate_invalid".into());
    }
    let port = port_text
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| "work.bridge_egress.apply_candidate_invalid".to_string())?;
    Ok(format!("{scheme}://{display_host}:{port}"))
}

fn build_partial_update_body(candidate_url: &str) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&serde_json::json!({
        "outbound_proxy_url": candidate_url,
    }))
    .map_err(|_| "work.bridge_egress.apply_request_invalid".to_string())
}

fn rollback_error(
    operations: &mut impl BridgeEgressApplyOps,
    backup: &BridgeConfigBackup,
    failure: String,
) -> String {
    match operations.restore_backup(backup) {
        Ok(()) => format!(
            "work.bridge_egress.apply_rolled_back: {failure}; restored {}",
            backup.backup_path
        ),
        Err(rollback_failure) => format!(
            "work.bridge_egress.apply_rolled_back: {failure}; rollback from {} was incomplete (work.bridge_egress.apply_rollback_failed: {})",
            backup.backup_path,
            clean_diagnostic_text(&rollback_failure)
        ),
    }
}

fn apply_with_operations(
    operations: &mut impl BridgeEgressApplyOps,
    candidate_url: &str,
) -> Result<BridgeEgressApplyReport, String> {
    let normalized_candidate = normalize_candidate_url(candidate_url)?;
    let body = build_partial_update_body(&normalized_candidate)?;
    let backup = operations.backup_config().map_err(|error| {
        format!(
            "work.bridge_egress.apply_backup_failed: {}",
            clean_diagnostic_text(&error)
        )
    })?;

    if let Err(error) = operations.post_partial_config(&backup, &body) {
        return Err(rollback_error(
            operations,
            &backup,
            format!(
                "work.bridge_egress.apply_post_failed: {}",
                clean_diagnostic_text(&error)
            ),
        ));
    }

    let after_url = match operations.read_health_outbound_proxy_url() {
        Ok(value) => value,
        Err(error) => {
            return Err(rollback_error(
                operations,
                &backup,
                format!(
                    "work.bridge_egress.apply_health_failed: {}",
                    clean_diagnostic_text(&error)
                ),
            ));
        }
    };
    if after_url != normalized_candidate {
        return Err(rollback_error(
            operations,
            &backup,
            format!(
                "work.bridge_egress.apply_health_mismatch: expected {}, got {}",
                display_candidate(&normalized_candidate),
                display_candidate(&after_url)
            ),
        ));
    }

    let after_probe = match operations.run_egress_probe() {
        Ok(report)
            if report.ok
                && report.code == "work.bridge_egress.ok"
                && report.request.state == BridgeEgressLayerState::Passed =>
        {
            report
        }
        Ok(report) => {
            return Err(rollback_error(
                operations,
                &backup,
                format!(
                    "work.bridge_egress.apply_probe_failed: {} (request={})",
                    report.code,
                    report.request.state.as_str()
                ),
            ));
        }
        Err(error) => {
            return Err(rollback_error(
                operations,
                &backup,
                format!(
                    "work.bridge_egress.apply_probe_failed: {}",
                    clean_diagnostic_text(&error)
                ),
            ));
        }
    };

    Ok(BridgeEgressApplyReport {
        operation: "bridge_egress_apply".into(),
        ok: true,
        code: "work.bridge_egress.apply_ok".into(),
        backup_path: backup.backup_path,
        before_outbound_proxy_url: backup.before_outbound_proxy_url,
        after_outbound_proxy_url: after_url,
        after_probe,
    })
}

fn display_candidate(value: &str) -> &str {
    if value.is_empty() {
        "direct"
    } else {
        value
    }
}

fn apply_bridge_egress_fix_impl(candidate_url: String) -> Result<BridgeEgressApplyReport, String> {
    let _transition = bridge_config_transition_lock().try_lock().map_err(|_| {
        "work.bridge_egress.apply_busy: another Bridge configuration transaction is active"
            .to_string()
    })?;
    let _service_operation = service_operation_lock("bridge-egress-apply")
        .map_err(|error| format!("work.bridge_egress.apply_busy: {error}"))?;
    let normalized_candidate = normalize_candidate_url(&candidate_url)?;
    if !consume_recent_candidate_proof(&normalized_candidate) {
        return Err(
            "work.bridge_egress.apply_candidate_unproven: rerun 能力体检 and select a recently proven candidate"
                .into(),
        );
    }
    let distros = discover_distros_with_timeout(APPLY_DISTRO_DISCOVERY_BUDGET)
        .map_err(|_| "work.bridge_egress.apply_wsl_unavailable".to_string())?;
    let distro = preferred_distro(&distros)
        .ok_or_else(|| "work.bridge_egress.apply_wsl_distro_missing".to_string())?;
    apply_with_operations(
        &mut WslBridgeEgressApplyOps { distro },
        &normalized_candidate,
    )
}

#[tauri::command]
pub async fn apply_bridge_egress_fix(
    candidate_url: String,
) -> Result<BridgeEgressApplyReport, String> {
    run_blocking(move || apply_bridge_egress_fix_impl(candidate_url))
        .await
        .map_err(|error| ensure_error_prefix("work.bridge_egress.apply_failed", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_fix_is_partial_update() {
        let body = build_partial_update_body("http://127.0.0.1:12334").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 1);
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["outbound_proxy_url"]
        );
        assert_eq!(
            object
                .get("outbound_proxy_url")
                .and_then(|value| value.as_str()),
            Some("http://127.0.0.1:12334")
        );
    }

    struct FailingProbeOps {
        current: String,
        saved: Option<String>,
        posted_bodies: Vec<Vec<u8>>,
        rollback_called: bool,
    }

    impl BridgeEgressApplyOps for FailingProbeOps {
        fn backup_config(&mut self) -> Result<BridgeConfigBackup, String> {
            self.saved = Some(self.current.clone());
            Ok(BridgeConfigBackup {
                config_path: "/home/test/.claude-science/proxy/config.json".into(),
                backup_path: "/home/test/.claude-science/proxy/config.json.bak-20260825-123456"
                    .into(),
                before_outbound_proxy_url: self.current.clone(),
            })
        }

        fn post_partial_config(
            &mut self,
            _backup: &BridgeConfigBackup,
            body: &[u8],
        ) -> Result<(), String> {
            self.posted_bodies.push(body.to_vec());
            let value: serde_json::Value = serde_json::from_slice(body).unwrap();
            self.current = value["outbound_proxy_url"].as_str().unwrap().to_string();
            Ok(())
        }

        fn read_health_outbound_proxy_url(&mut self) -> Result<String, String> {
            Ok(self.current.clone())
        }

        fn run_egress_probe(&mut self) -> Result<BridgeEgressReport, String> {
            Err("work.bridge_egress.upstream_502".into())
        }

        fn restore_backup(&mut self, _backup: &BridgeConfigBackup) -> Result<(), String> {
            self.rollback_called = true;
            self.current = self.saved.clone().unwrap();
            Ok(())
        }
    }

    #[test]
    fn apply_fix_rolls_back_on_failure() {
        let original = "http://127.0.0.1:10808".to_string();
        let mut operations = FailingProbeOps {
            current: original.clone(),
            saved: None,
            posted_bodies: Vec::new(),
            rollback_called: false,
        };
        let error = apply_with_operations(&mut operations, "http://127.0.0.1:12334")
            .expect_err("a failed verification must roll back");
        assert!(error.starts_with("work.bridge_egress.apply_rolled_back:"));
        assert!(error.contains("config.json.bak-20260825-123456"));
        assert!(operations.rollback_called);
        assert_eq!(operations.current, original);
        assert_eq!(operations.posted_bodies.len(), 1);
    }

    #[test]
    fn candidate_validation_accepts_discovered_proxy_or_direct() {
        assert_eq!(normalize_candidate_url("").unwrap(), "");
        assert_eq!(normalize_candidate_url("direct").unwrap(), "");
        assert_eq!(
            normalize_candidate_url("HTTP://LOCALHOST:12334").unwrap(),
            "http://localhost:12334"
        );
        assert_eq!(
            normalize_candidate_url("socks5h://[::1]:7890").unwrap(),
            "socks5h://[::1]:7890"
        );
        assert_eq!(
            normalize_candidate_url("http://192.168.1.10:7890").unwrap(),
            "http://192.168.1.10:7890"
        );
        assert_eq!(
            normalize_candidate_url("https://proxy.example.com:8443").unwrap(),
            "https://proxy.example.com:8443"
        );
        assert!(normalize_candidate_url("http://user:secret@127.0.0.1:7890").is_err());
        assert!(normalize_candidate_url("http://127.0.0.1:0").is_err());
    }

    #[test]
    fn wsl_transport_passes_only_lowercase_hex_argv_and_python_over_stdin() {
        let hostile = b"http://127.0.0.1:7890/$(*? token)";
        let encoded = hex_encode(hostile);
        assert!(encoded
            .bytes()
            .all(|value| value.is_ascii_digit() || matches!(value, b'a'..=b'f')));
        assert!(!encoded.contains('*'));
        assert!(!encoded.contains('$'));
        assert!(!encoded.contains(' '));

        let source = include_str!("bridge_egress_apply.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains(".arg(\"--\")"));
        assert!(production.contains(".arg(\"timeout\")"));
        assert!(production.contains(".arg(\"--kill-after=1s\")"));
        assert!(production.contains(".arg(\"python3\")"));
        assert!(production.contains(".arg(\"-\")"));
        assert!(production.contains("listener_pids != {pid}"));
        assert!(production.contains("single-field live Bridge rollback failed"));
        assert!(RESTORE_CONFIG_SCRIPT.contains("mask_url_credentials(original_proxy)"));
        assert!(RESTORE_CONFIG_SCRIPT.contains("f\"****@{host}\""));
        assert!(!RESTORE_CONFIG_SCRIPT
            .contains("health_after.get(\"outbound_proxy_url\") != original_proxy"));
        assert!(!RESTORE_CONFIG_SCRIPT.contains("os.replace"));
        assert!(!RESTORE_CONFIG_SCRIPT.contains("tempfile"));
        assert!(!production.contains("bash -lc"));
    }
}
