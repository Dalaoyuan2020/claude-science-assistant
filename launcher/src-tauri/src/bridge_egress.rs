use super::{
    background_command, command_output_with_stdin_timeout, discover_distros_with_timeout,
    output_text, preferred_distro,
};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const BRIDGE_EGRESS_WATCHDOG: Duration = Duration::from_secs(64);
const BRIDGE_EGRESS_CONNECTION_WATCHDOG: Duration = Duration::from_secs(10);
const DISTRO_DISCOVERY_BUDGET: Duration = Duration::from_millis(1_500);
const CANDIDATE_PROOF_MAX_AGE: Duration = Duration::from_secs(5 * 60);
const GUEST_PROCESS_RESERVE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedBridgeProofContext {
    distro: String,
    runtime_pid: u32,
    runtime_source_sha256: String,
    runtime_starttime: String,
}

#[derive(Default)]
struct CandidateProofCache {
    recorded_at: Option<Instant>,
    addresses: Vec<String>,
    upstream_origin: Option<String>,
    managed_context: Option<ManagedBridgeProofContext>,
}

#[derive(Debug, Clone)]
struct CandidateProofEvidence {
    upstream_origin: String,
    managed_context: ManagedBridgeProofContext,
}

static CANDIDATE_PROOFS: OnceLock<Mutex<CandidateProofCache>> = OnceLock::new();

fn candidate_proofs() -> &'static Mutex<CandidateProofCache> {
    CANDIDATE_PROOFS.get_or_init(|| Mutex::new(CandidateProofCache::default()))
}

const BRIDGE_EGRESS_PROBE_PYTHON: &str = r#"
import hashlib
import http.client
import json
import os
import re
import socket
import ssl
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from urllib.parse import quote, urlsplit, urlunsplit

PREFIX = "work.bridge_egress."
runtime_pid = None
runtime_source_sha256 = None
runtime_starttime = None
PREFERRED_MODEL = "claude-haiku-4-5-20251001"
PROBE_DEADLINE = time.monotonic() + 56.0
MAX_LOCAL_BODY = 1024 * 1024
confirm_billable = len(sys.argv) >= 2 and sys.argv[1] == "1"
requested_models = []
for requested in sys.argv[2:]:
    if (re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}", requested or "") is not None
            and requested not in requested_models):
        requested_models.append(requested)


def code(name):
    return PREFIX + name


def elapsed_ms(started):
    return max(0, int((time.monotonic() - started) * 1000))


def bounded_timeout(requested):
    return max(0.05, min(requested, PROBE_DEADLINE - time.monotonic()))


def make_layer(state, name, started=None, http_status=None, detail=None):
    value = {
        "state": state,
        "code": code(name),
        "durationMs": elapsed_ms(started) if started is not None else 0,
    }
    if isinstance(http_status, int):
        value["httpStatus"] = http_status
    if detail:
        value["detail"] = detail
    return value


health_layer = make_layer("skipped", "health_skipped")
proxy_layer = make_layer("skipped", "proxy_skipped")
models_layer = make_layer("skipped", "models_skipped")
request_layer = make_layer("skipped", "request_skipped")
direct_layer = make_layer("skipped", "direct_skipped")
model = None
outbound_proxy_configured = False
outbound_proxy_url = None
upstream_base_url = None
billable_request_sent = False
warnings = []
candidates = []


def emit(top_name, conclusion, suggested_action, ok=False):
    report = {
        "operation": "bridge_egress",
        "ok": bool(ok),
        "code": code(top_name),
        "conclusion": conclusion,
        "billableRequestSent": bool(billable_request_sent),
        "outboundProxyConfigured": bool(outbound_proxy_configured),
        "health": health_layer,
        "proxy": proxy_layer,
        "models": models_layer,
        "request": request_layer,
        "direct": direct_layer,
        "candidates": candidates,
        "suggestedAction": suggested_action,
        "warnings": warnings[:8],
    }
    if model:
        report["model"] = model
    if outbound_proxy_url:
        report["outboundProxyUrl"] = outbound_proxy_url
    if upstream_base_url:
        report["upstreamBaseUrl"] = upstream_base_url
    if runtime_pid:
        report["runtimePid"] = runtime_pid
    if runtime_source_sha256:
        report["runtimeSourceSha256"] = runtime_source_sha256
    if runtime_starttime:
        report["runtimeStarttime"] = runtime_starttime
    print(json.dumps(report, ensure_ascii=True, separators=(",", ":")))
    raise SystemExit(0)


def local_get(path, timeout):
    connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=bounded_timeout(timeout))
    try:
        connection.request("GET", path, headers={"Connection": "close"})
        response = connection.getresponse()
        return response.status, response.read(MAX_LOCAL_BODY + 1)
    finally:
        connection.close()


def valid_managed_identity(identity):
    if not isinstance(identity, dict):
        return False
    runtime_id = identity.get("runtimeId")
    version = identity.get("version")
    build_id = identity.get("buildId")
    source_path = identity.get("sourcePath")
    source_sha256 = identity.get("sourceSha256")
    capabilities = identity.get("capabilities")
    pid = identity.get("pid")
    return (
        identity.get("schemaVersion") == 1
        and identity.get("component") == "bridge"
        and identity.get("managed") is True
        and isinstance(runtime_id, str)
        and re.fullmatch(r"[A-Za-z0-9._-]+", runtime_id or "") is not None
        and isinstance(version, str) and bool(version.strip())
        and isinstance(build_id, str)
        and re.fullmatch(r"[0-9A-Fa-f]{16}", build_id or "") is not None
        and isinstance(source_path, str) and bool(source_path.strip())
        and isinstance(source_sha256, str)
        and re.fullmatch(r"[0-9A-Fa-f]{64}", source_sha256 or "") is not None
        and build_id.casefold() == source_sha256[:16].casefold()
        and isinstance(pid, int) and not isinstance(pid, bool) and pid > 0
        and isinstance(capabilities, list) and "health" in capabilities
    )


def valid_current_bridge_identity(identity):
    try:
        current_proxy = (Path.home() / ".local" / "share" / "csa" / "runtime" /
                         "bridge" / "current" / "proxy.py")
        source_path = Path(identity["sourcePath"])
        source_hash = hashlib.sha256(current_proxy.read_bytes()).hexdigest()
        identity_pid = int(identity["pid"])
        sockets = subprocess.run(
            ["ss", "-H", "-ltnp", "sport = :9876"],
            capture_output=True, text=True, timeout=0.8, check=False,
        )
        listener_pids = set(int(value) for value in re.findall(r"pid=(\d+)", sockets.stdout))
        argv = (Path("/proc") / str(identity_pid) / "cmdline").read_bytes().split(b"\0")
        argv_proxy_paths = []
        for item in argv:
            try:
                value = item.decode("utf-8")
                if value.endswith("proxy.py"):
                    argv_proxy_paths.append(Path(value).resolve())
            except Exception:
                pass
        return (
            current_proxy.resolve() == source_path.resolve()
            and source_hash.casefold() == str(identity["sourceSha256"]).casefold()
            and listener_pids == {identity_pid}
            and current_proxy.resolve() in argv_proxy_paths
        )
    except Exception:
        return False


def process_starttime(pid):
    try:
        value = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
        tail = value[value.rfind(")") + 2:].split()
        starttime = tail[19]
        return starttime if re.fullmatch(r"\d{1,32}", starttime or "") else None
    except Exception:
        return None


def clean_host(host):
    if not isinstance(host, str) or not host or any(ord(ch) < 33 for ch in host):
        return None
    return host


def display_host(host):
    return "[" + host + "]" if ":" in host and not host.startswith("[") else host


def proxy_endpoint(raw):
    try:
        parts = urlsplit(str(raw or ""))
        scheme = parts.scheme.lower()
        if scheme == "socks":
            scheme = "socks5h"
        host = clean_host(parts.hostname)
        if scheme not in {"http", "https", "socks5", "socks5h"} or not host:
            return None
        defaults = {"http": 80, "https": 443, "socks5": 1080, "socks5h": 1080}
        port = parts.port or defaults[scheme]
        if port < 1 or port > 65535:
            return None
        return {
            "host": host,
            "port": port,
            "sanitized": f"{scheme}://{display_host(host)}:{port}",
        }
    except Exception:
        return None


def upstream_endpoint(raw):
    try:
        parts = urlsplit(str(raw or ""))
        scheme = parts.scheme.lower()
        host = clean_host(parts.hostname)
        if (scheme not in {"http", "https"} or not host
                or parts.username is not None or parts.password is not None
                or parts.query or parts.fragment):
            return None
        port = parts.port
        if port is not None and (port < 1 or port > 65535):
            return None
        path = parts.path or "/"
        if (not path.startswith("/") or '"' in path or "\\" in path
                or any(ord(ch) < 33 or ch.isspace() for ch in path)):
            return None
        netloc = display_host(host) + ((":" + str(port)) if port is not None else "")
        # Keep the configured path private but probe it: the public report only
        # exposes the origin, while curl receives the full target over stdin so
        # a path never enters /proc/<pid>/cmdline. URLs with userinfo, query, or
        # fragment are rejected rather than replaying possible secrets.
        sanitized = urlunsplit((scheme, netloc, "", "", ""))
        return {
            "scheme": scheme,
            "host": host,
            "port": port or (443 if scheme == "https" else 80),
            "path": path,
            "sanitized": sanitized,
        }
    except Exception:
        return None


WINDOWS_CANDIDATE_DISCOVERY_PS = r"""
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$knownPorts = @(7890, 7897, 10808, 10809, 12334, 20171, 1080, 8080)
$settings = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
$listeners = @(
  Get-NetTCPConnection -State Listen -LocalPort $knownPorts | ForEach-Object {
    $processName = $null
    if ($_.OwningProcess) {
      $processName = (Get-Process -Id $_.OwningProcess).ProcessName
    }
    [pscustomobject]@{
      port = [int]$_.LocalPort
      process = $processName
    }
  }
)
[pscustomobject]@{
  proxyEnabled = ($settings.ProxyEnable -eq 1)
  proxyServer = [string]$settings.ProxyServer
  listeners = $listeners
} | ConvertTo-Json -Compress -Depth 4
"""


def normalize_windows_proxy(raw):
    value = str(raw or "").strip()
    if not value:
        return None
    entries = {}
    if ";" in value or ("=" in value and "://" not in value):
        for item in value.split(";"):
            if "=" not in item:
                continue
            name, endpoint_value = item.split("=", 1)
            entries[name.strip().lower()] = endpoint_value.strip()
        value = entries.get("https") or entries.get("http") or ""
        default_scheme = "http"
        if not value and entries.get("socks"):
            value = entries["socks"]
            default_scheme = "socks5h"
    else:
        default_scheme = "http"
    if not value:
        return None
    if "://" not in value:
        value = default_scheme + "://" + value
    endpoint = proxy_endpoint(value)
    return endpoint["sanitized"] if endpoint is not None else None


def read_windows_proxy_candidates():
    try:
        completed = subprocess.run(
            ["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-File", "-"],
            input=WINDOWS_CANDIDATE_DISCOVERY_PS + "\n\n",
            capture_output=True,
            text=True,
            timeout=bounded_timeout(3.0),
            check=False,
        )
        payload_line = next(
            (line.strip() for line in reversed(completed.stdout.splitlines())
             if line.strip().startswith("{")),
            "",
        )
        payload = json.loads(payload_line) if payload_line else {}
        if not isinstance(payload, dict):
            warnings.append("Windows 候选发现未返回结构化结果；候选列表仍保留直连对照。")
            return None, []
        system_proxy = normalize_windows_proxy(payload.get("proxyServer")) \
            if payload.get("proxyEnabled") is True else None
        raw_listeners = payload.get("listeners")
        if isinstance(raw_listeners, dict):
            raw_listeners = [raw_listeners]
        listeners = []
        for item in raw_listeners if isinstance(raw_listeners, list) else []:
            if not isinstance(item, dict):
                continue
            try:
                port = int(item.get("port"))
            except Exception:
                continue
            if port not in {7890, 7897, 10808, 10809, 12334, 20171, 1080, 8080}:
                continue
            process_name = str(item.get("process") or "").strip()
            if not re.fullmatch(r"[A-Za-z0-9._ -]{1,96}", process_name):
                process_name = None
            listeners.append({
                "address": f"http://127.0.0.1:{port}",
                "processName": process_name,
            })
        return system_proxy, listeners
    except Exception:
        warnings.append("无法读取 Windows 系统代理与常用监听端口；候选列表仍保留直连对照。")
        return None, []


def candidate_http_probe(address, endpoint, timeout):
    if endpoint is None:
        return None, "unreachable"
    netloc = display_host(endpoint["host"])
    default_port = 443 if endpoint["scheme"] == "https" else 80
    if endpoint["port"] != default_port:
        netloc += ":" + str(endpoint["port"])
    target = urlunsplit((endpoint["scheme"], netloc, endpoint["path"], "", ""))
    try:
        command = [
            "curl", "--disable", "--silent", "--show-error", "--output", "/dev/null",
            "--write-out", "%{http_code}", "--connect-timeout", "0.75",
            "--max-time", str(max(1.0, min(timeout, bounded_timeout(timeout)))),
            "--user-agent", "CSA-Bridge-Egress-Candidate/1", "--config", "-",
        ]
        if address == "direct":
            command.extend(["--proxy", ""])
        else:
            command.extend(["--noproxy", "", "--proxy", address])
        curl_config = f'url = "{target}"\n'
        completed = subprocess.run(
            command, input=curl_config, capture_output=True, text=True,
            timeout=bounded_timeout(timeout + 0.5), check=False,
        )
        status_text = completed.stdout.strip()
        status = int(status_text) if re.fullmatch(r"\d{3}", status_text) else None
        # A bounded HTTP response, including 401/403, proves the upstream HTTP layer answered.
        if status in {407, 502, 503, 504}:
            return None, "upstream_error"
        if isinstance(status, int) and status > 0:
            return status, None
        if completed.returncode == 28:
            return None, "timeout"
        return None, "unreachable"
    except (TimeoutError, socket.timeout):
        return None, "timeout"
    except Exception:
        return None, "unreachable"


def probe_candidate(definition, upstream):
    address = definition["address"]
    tcp_started = time.monotonic()
    if address == "direct":
        tcp_target = (upstream["host"], upstream["port"]) if upstream is not None else None
    else:
        proxy = proxy_endpoint(address)
        tcp_target = (proxy["host"], proxy["port"]) if proxy is not None else None
    try:
        if tcp_target is None:
            raise OSError("candidate target unavailable")
        connection = socket.create_connection(tcp_target, timeout=bounded_timeout(0.75))
        connection.close()
        tcp = make_layer("passed", "candidate.tcp_ok", tcp_started,
                         detail="Candidate accepted a bounded TCP connection.")
    except Exception:
        tcp = make_layer("failed", "candidate.unreachable", tcp_started,
                         detail="Candidate did not accept a bounded TCP connection.")
        return {
            **definition,
            "tcp": tcp,
            "upstream": make_layer("skipped", "candidate.upstream_skipped",
                                   detail="Upstream check was skipped because TCP failed."),
            "recommended": False,
            "reason": "TCP 不可达，不能作为 Bridge 出口。",
        }

    upstream_started = time.monotonic()
    status, failure = candidate_http_probe(address, upstream, 2.0)
    if (not isinstance(status, int)
            and definition["source"] == "listening_port"
            and address.startswith("http://")):
        socks_address = "socks5h://" + address[len("http://"):]
        socks_status, socks_failure = candidate_http_probe(socks_address, upstream, 2.0)
        if isinstance(socks_status, int):
            definition = {**definition, "address": socks_address}
            address = socks_address
            status, failure = socks_status, None
        elif failure != "timeout":
            failure = socks_failure
    if isinstance(status, int):
        upstream_layer = make_layer(
            "passed", "candidate.upstream_ok", upstream_started,
            http_status=status,
            detail="Candidate reached the current configured upstream base-URL path without a Key; this does not prove a model request.",
        )
        if definition["source"] == "windows_system_proxy":
            reason = "它是当前 Windows 系统代理，且能到达当前配置的上游 base URL；模型请求仍由应用后真实验证。"
        elif address == "direct":
            reason = "直连能到达当前配置的上游 base URL，但对 OpenAI / Anthropic 的覆盖可能较窄；这不等同模型请求已验证。"
        elif definition.get("processName"):
            reason = f"本地进程 {definition['processName']} 正在监听，且能到达当前配置的上游 base URL；模型请求待应用后验证。"
        else:
            reason = "本地常用代理端口正在监听，且能到达当前配置的上游 base URL；模型请求待应用后验证。"
    else:
        detail = ("Candidate upstream check timed out." if failure == "timeout"
                  else "Candidate could not reach the current upstream HTTP service.")
        upstream_layer = make_layer(
            "failed", "candidate.upstream_unreachable", upstream_started, detail=detail,
        )
        reason = "TCP 可达，但当前上游不可达，不能作为推荐出口。"
    return {
        **definition,
        "tcp": tcp,
        "upstream": upstream_layer,
        "recommended": False,
        "reason": reason,
    }


def discover_bridge_candidates(upstream):
    system_proxy, listeners = read_windows_proxy_candidates()
    definitions = []
    seen = set()

    def add(address, source, process_name=None):
        identity = address.casefold()
        if identity in seen:
            existing = next(item for item in definitions if item["address"].casefold() == identity)
            if source == "windows_system_proxy":
                existing["source"] = source
            if process_name and not existing.get("processName"):
                existing["processName"] = process_name
            return
        seen.add(identity)
        definitions.append({
            "address": address,
            "source": source,
            "processName": process_name,
        })

    if system_proxy:
        add(system_proxy, "windows_system_proxy")
    for listener in listeners:
        add(listener["address"], "listening_port", listener.get("processName"))
    add("direct", "direct")

    with ThreadPoolExecutor(max_workers=min(8, len(definitions))) as executor:
        reports = list(executor.map(lambda item: probe_candidate(item, upstream), definitions))
    merged = {}
    for report in reports:
        identity = report["address"].casefold()
        existing = merged.get(identity)
        if existing is None:
            merged[identity] = report
            continue
        if report["upstream"]["state"] == "passed" and existing["upstream"]["state"] != "passed":
            report, existing = existing, report
            merged[identity] = existing
        if existing["source"] != "windows_system_proxy" and report["source"] == "windows_system_proxy":
            existing["source"] = "windows_system_proxy"
        if not existing.get("processName") and report.get("processName"):
            existing["processName"] = report["processName"]
    reports = list(merged.values())
    for report in reports:
        if report["upstream"]["state"] == "passed":
            if report["source"] == "windows_system_proxy":
                report["reason"] = "它是当前 Windows 系统代理，且能到达当前配置的上游 base URL；模型请求仍由应用后真实验证。"
            elif report["address"] == "direct":
                report["reason"] = "直连能到达当前配置的上游 base URL，但对 OpenAI / Anthropic 的覆盖可能较窄；这不等同模型请求已验证。"
    reports.sort(key=lambda item: (
        item["upstream"]["state"] != "passed",
        item["source"] == "direct",
        item["source"] != "windows_system_proxy",
        item["address"],
    ))
    recommended = next((item for item in reports if item["upstream"]["state"] == "passed"), None)
    if recommended is not None:
        recommended["recommended"] = True
    return reports


def probe_configured_proxy(endpoint):
    started = time.monotonic()
    if endpoint is None:
        return (
            make_layer("failed", "proxy_dead", started,
                       detail="Configured outbound proxy endpoint is invalid after redaction."),
            "Bridge 配置了无法安全解析的 outbound proxy；已短路，未发送真实模型请求。",
            "从候选列表选择能够到达当前上游的出口；不要自动修改系统网络。",
        )
    try:
        connection = socket.create_connection(
            (endpoint["host"], endpoint["port"]),
            timeout=bounded_timeout(1.5),
        )
        connection.close()
        return (
            make_layer("passed", "proxy_ok", started,
                       detail="Configured proxy accepted a WSL TCP connection."),
            None,
            None,
        )
    except Exception:
        return (
            make_layer("failed", "proxy_dead", started,
                       detail="Configured proxy did not accept a WSL TCP connection within 1.5 seconds."),
            "Bridge 配置的 outbound proxy 在 WSL 内未监听或不可达；已短路，未发送真实模型请求。",
            "从候选列表选择能够到达当前上游的出口；不要自动修改系统网络。",
        )


health_started = time.monotonic()
try:
    health_status, health_body = local_get("/health", 2.0)
except (TimeoutError, socket.timeout):
    health_layer = make_layer("failed", "health_unreachable", health_started,
                              detail="Bridge /health timed out inside WSL.")
    emit("health_unreachable", "WSL 内无法在 2 秒内访问受管 Bridge /health；未发送真实模型请求。",
         "先恢复受管 WSL Bridge，再重跑出口体检。")
except Exception:
    health_layer = make_layer("failed", "health_unreachable", health_started,
                              detail="Bridge /health is unreachable inside WSL.")
    emit("health_unreachable", "WSL 内无法访问受管 Bridge /health；未发送真实模型请求。",
         "先恢复受管 WSL Bridge，再重跑出口体检。")

if health_status != 200:
    health_layer = make_layer("failed", "health_unreachable", health_started,
                              http_status=health_status,
                              detail="Bridge /health did not return HTTP 200.")
    emit("health_unreachable", "Bridge /health 未返回成功状态；未发送真实模型请求。",
         "先恢复受管 WSL Bridge，再重跑出口体检。")

try:
    health = json.loads(health_body.decode("utf-8"))
except Exception:
    health = None
if (not isinstance(health, dict)
        or not valid_managed_identity(health.get("runtime_identity"))
        or not valid_current_bridge_identity(health.get("runtime_identity"))):
    health_layer = make_layer("failed", "health_identity_invalid", health_started,
                              http_status=health_status,
                              detail="Bridge did not present a valid managed runtime identity.")
    emit("health_identity_invalid", "9876 端口未提供有效的 CSA 受管 Bridge 身份；未发送真实模型请求。",
         "先用启动器修复受管 Bridge 身份，禁止向未知本地服务发送模型请求。")

identity = health["runtime_identity"]
identity_starttime = process_starttime(identity["pid"])
if identity_starttime is None:
    health_layer = make_layer("failed", "health_identity_invalid", health_started,
                              http_status=health_status,
                              detail="Bridge process start time is invalid.")
    emit("health_identity_invalid", "9876 端口的受管 Bridge 上下文不完整；未发送真实模型请求。",
         "先刷新受管 Bridge 身份，再重跑出口体检。")
runtime_pid = identity["pid"]
runtime_source_sha256 = str(identity["sourceSha256"]).lower()
runtime_starttime = identity_starttime
health_layer = make_layer("passed", "health_ok", health_started, http_status=health_status)
outbound_proxy_configured = health.get("outbound_proxy_configured") is True
proxy_failure_conclusion = None
proxy_failure_action = None

if outbound_proxy_configured:
    endpoint = proxy_endpoint(health.get("outbound_proxy_url"))
    if endpoint is not None:
        outbound_proxy_url = endpoint["sanitized"]
    proxy_layer, proxy_failure_conclusion, proxy_failure_action = probe_configured_proxy(endpoint)
else:
    proxy_layer = make_layer("skipped", "proxy_not_configured",
                             detail="Bridge has no configured outbound proxy.")


def read_bridge_environment(pid):
    values = {}
    try:
        payload = (Path("/proc") / str(pid) / "environ").read_bytes()
        for item in payload.split(b"\0"):
            if b"=" not in item:
                continue
            key, value = item.split(b"=", 1)
            values[key.decode("utf-8", "ignore")] = value.decode("utf-8", "ignore")
    except Exception:
        pass
    return values


def read_bridge_config(bridge_environment):
    configured_root = str(bridge_environment.get("CLAUDE_SCIENCE_PROXY_DIR") or "").strip()
    config_path = (Path(configured_root).expanduser() if configured_root
                   else Path.home() / ".claude-science" / "proxy") / "config.json"
    try:
        if config_path.stat().st_size > 4 * 1024 * 1024:
            return {}
        value = json.loads(config_path.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}
    except Exception:
        return {}


bridge_environment = read_bridge_environment(identity["pid"])
bridge_config = read_bridge_config(bridge_environment)


def effective_text(config_name, environment_name, default=""):
    env_value = bridge_environment.get(environment_name)
    if env_value not in (None, ""):
        return str(env_value)
    value = bridge_config.get(config_name, default)
    return str(value) if value is not None else str(default)


def config_json_value(config_name, environment_name, default):
    env_value = bridge_environment.get(environment_name)
    if env_value not in (None, ""):
        try:
            return json.loads(env_value)
        except Exception:
            return default
    return bridge_config.get(config_name, default)


def normalized_aliases(raw):
    if isinstance(raw, dict):
        items = []
        for alias_id, value in raw.items():
            item = dict(value) if isinstance(value, dict) else {"model": value}
            item.setdefault("id", alias_id)
            items.append(item)
    elif isinstance(raw, list):
        items = raw
    else:
        items = []
    return [item for item in items if isinstance(item, dict) and str(item.get("id") or "").strip()]


def normalize_upstream_base(raw, mode):
    base = str(raw or "").strip().rstrip("/")
    if not base:
        return ""
    if mode == "anthropic":
        if base.endswith("/v1"):
            return base
        if base.endswith("/anthropic"):
            return base + "/v1"
        try:
            host = (urlsplit(base).hostname or "").lower()
        except Exception:
            host = ""
        if host == "api.deepseek.com" and "/anthropic" not in urlsplit(base).path:
            return base + "/anthropic/v1"
        return base + "/v1"
    if base.endswith("/v1") or base.endswith("/v4"):
        return base
    return base + "/v1"


def resolve_upstream_base(selected_model):
    aliases = normalized_aliases(config_json_value("model_aliases", "MODEL_ALIASES", []))
    alias = next((item for item in aliases if str(item.get("id") or "").strip() == selected_model), None)
    route_id = str((alias or {}).get("route_id") or "").strip()
    if route_id:
        routes = bridge_config.get("aggregate_upstreams", [])
        route = next((item for item in routes if isinstance(item, dict)
                      and str(item.get("id") or "").strip() == route_id), None) \
            if isinstance(routes, list) else None
        if route:
            mode = str(route.get("mode") or "openai").strip().lower()
            return normalize_upstream_base(route.get("base_url"), mode)

    backend = str((alias or {}).get("backend") or
                  effective_text("default_backend", "DEFAULT_BACKEND", "")).strip().lower()
    force_model = effective_text("force_model", "FORCE_MODEL", "").strip()
    if alias is None and not force_model:
        patterns = [
            ("deepseek", effective_text("deepseek_model_pattern", "DEEPSEEK_MODEL_PATTERN", r"deepseek|deep-seek")),
            ("openai", effective_text("openai_model_pattern", "OPENAI_MODEL_PATTERN", r"^(gpt-|o1|o3|o4|chatgpt)")),
            ("custom", effective_text("custom_model_pattern", "CUSTOM_MODEL_PATTERN", "")),
        ]
        for candidate, pattern in patterns:
            if not pattern:
                continue
            try:
                if re.search(pattern, selected_model, re.IGNORECASE):
                    backend = candidate
                    break
            except Exception:
                continue

    defaults = {
        "deepseek": ("https://api.deepseek.com/anthropic", "anthropic"),
        "openai": ("https://api.openai.com", "openai"),
        "custom": ("", "openai"),
    }
    if backend not in defaults:
        return ""
    default_base, default_mode = defaults[backend]
    raw_base = effective_text(backend + "_base_url", backend.upper() + "_BASE_URL", default_base)
    mode = effective_text(backend + "_upstream_mode", backend.upper() + "_UPSTREAM_MODE", default_mode).strip().lower()
    return normalize_upstream_base(raw_base, "anthropic" if mode == "anthropic" else "openai")


if proxy_failure_conclusion is not None:
    configured_aliases = normalized_aliases(config_json_value("model_aliases", "MODEL_ALIASES", []))
    configured_model = effective_text("force_model", "FORCE_MODEL", "").strip()
    if not configured_model:
        preferred_alias = next(
            (item for item in configured_aliases if str(item.get("id") or "").strip() == PREFERRED_MODEL),
            configured_aliases[0] if configured_aliases else None,
        )
        configured_model = str((preferred_alias or {}).get("id") or "").strip()
    effective_upstream = resolve_upstream_base(configured_model)
    upstream = upstream_endpoint(effective_upstream)
    if upstream is not None:
        upstream_base_url = upstream["sanitized"]
    else:
        warnings.append("无法从 Bridge 进程环境与 config.json 安全解析有效的上游 origin。")
    candidates = discover_bridge_candidates(upstream)
    emit("proxy_dead", proxy_failure_conclusion, proxy_failure_action)


health_auth_mode = str(health.get("proxy_auth_mode") or "optional").strip().lower()
effective_auth_mode = effective_text("proxy_auth_mode", "PROXY_AUTH_MODE", "optional").strip().lower()
proxy_auth_token = effective_text("proxy_auth_token", "PROXY_AUTH_TOKEN", "").strip()
if effective_auth_mode != health_auth_mode:
    models_layer = make_layer("failed", "models_failed",
                              detail="Local auth mode differs between health and the Bridge process configuration.")
    emit("models_failed", "Bridge 本地鉴权状态不一致，无法安全访问 /v1/models；未发送真实模型请求。",
         "只读核对 Bridge 进程环境与 config.json 的 proxy_auth_mode，然后重启 Bridge。")
if health_auth_mode == "required" and (
        health.get("proxy_auth_configured") is not True or not proxy_auth_token):
    models_layer = make_layer("failed", "models_failed",
                              detail="Required local Bridge auth secret is unavailable inside WSL.")
    emit("models_failed", "Bridge 要求本地路径鉴权，但 WSL 内无法取得有效 secret；未发送真实模型请求。",
         "修复 Bridge 自己的本地鉴权配置；不要把 secret 复制到 Windows 输出或日志。")

models_path = "/v1/models"
if health_auth_mode == "required":
    models_path = "/" + quote(proxy_auth_token, safe="") + models_path

models_started = time.monotonic()
if (not valid_current_bridge_identity(identity)
        or process_starttime(identity["pid"]) != identity_starttime):
    models_layer = make_layer("failed", "models_failed", models_started,
                              detail="Managed Bridge identity changed before the local models request.")
    emit("health_identity_changed", "受管 Bridge 身份在体检期间发生变化；已在发送本地鉴权路径前停止，未发送真实模型请求。",
         "刷新状态并确认 9876 仍由当前受管 Bridge 独占后重试。")
try:
    models_status, models_body = local_get(models_path, 4.0)
except Exception:
    models_layer = make_layer("failed", "models_failed", models_started,
                              detail="Bridge /v1/models is unreachable inside WSL.")
    emit("models_failed", "Bridge /v1/models 不可用；未发送真实模型请求。",
         "先修复 Bridge 本地模型列表与路径鉴权，再重跑出口体检。")

if models_status != 200:
    models_layer = make_layer("failed", "models_failed", models_started,
                              http_status=models_status,
                              detail="Bridge /v1/models did not return HTTP 200.")
    emit("models_failed", "Bridge /v1/models 未返回成功状态；未发送真实模型请求。",
         "先修复 Bridge 本地模型列表与路径鉴权，再重跑出口体检。")

try:
    models_payload = json.loads(models_body.decode("utf-8"))
    model_items = models_payload.get("data") if isinstance(models_payload, dict) else None
    model_ids = [
        item.get("id") for item in model_items
        if isinstance(item, dict)
        and isinstance(item.get("id"), str)
        and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}", item.get("id")) is not None
    ] if isinstance(model_items, list) else []
except Exception:
    model_ids = []
if not model_ids:
    models_layer = make_layer("failed", "models_failed", models_started,
                              http_status=models_status,
                              detail="Bridge /v1/models returned no usable model IDs.")
    emit("models_failed", "Bridge /v1/models 未返回可用模型；未发送真实模型请求。",
         "先在 Bridge 配置中建立有效模型映射，再重跑出口体检。")

missing_requested_models = [item for item in requested_models if item not in model_ids]
if missing_requested_models:
    models_layer = make_layer("failed", "models_failed", models_started,
                              http_status=models_status,
                              detail="The explicitly requested verification model is absent from /v1/models.")
    emit("models_failed", "Bridge /v1/models 未返回本次事务要求验证的模型；未发送真实模型请求。",
         "回滚本次切换，并核对聚合别名与三条路由。")
model = requested_models[0] if requested_models else (
    PREFERRED_MODEL if PREFERRED_MODEL in model_ids else model_ids[0]
)
models_layer = make_layer("passed", "models_ok", models_started,
                          http_status=models_status,
                          detail=f"Usable model count: {len(model_ids)}.")


effective_upstream = resolve_upstream_base(model)
upstream = upstream_endpoint(effective_upstream)
if upstream is not None:
    upstream_base_url = upstream["sanitized"]
else:
    warnings.append("无法从 Bridge 进程环境与 config.json 安全解析有效的上游 base URL。")

if not confirm_billable:
    request_layer = make_layer("skipped", "request_skipped",
                               detail="A billable request was not explicitly confirmed.")
    direct_layer = make_layer("skipped", "direct_skipped")
    emit("confirmation_required", "非计费预检已完成；未获确认，因此严格跳过 /v1/messages。",
         "如需验证真实 Bridge 出口，请明确确认一次 max_tokens=1 的模型请求。")


def local_post(path, body, timeout):
    connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=bounded_timeout(timeout))
    sent = False
    try:
        connection.connect()
        sent = True
        connection.request(
            "POST",
            path,
            body=body,
            headers={"Content-Type": "application/json", "Connection": "close"},
        )
        response = connection.getresponse()
        return response.status, response.read(MAX_LOCAL_BODY + 1), sent, None
    except (TimeoutError, socket.timeout):
        return None, b"", sent, "timeout"
    except Exception:
        return None, b"", sent, "unreachable"
    finally:
        connection.close()


messages_path = "/v1/messages"
if health_auth_mode == "required":
    messages_path = "/" + quote(proxy_auth_token, safe="") + messages_path
request_started = time.monotonic()
if (not valid_current_bridge_identity(identity)
        or process_starttime(identity["pid"]) != identity_starttime):
    request_layer = make_layer("skipped", "request_skipped", request_started,
                               detail="Managed Bridge identity changed before the billable local request.")
    direct_layer = make_layer("skipped", "direct_skipped")
    emit("health_identity_changed", "受管 Bridge 身份在体检期间发生变化；已在发送本地鉴权路径与真实请求前停止。",
         "刷新状态并确认 9876 仍由当前受管 Bridge 独占后重试。")


def verify_model(selected_model):
    body = json.dumps({
        "model": selected_model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "Reply with one short word."}],
        "stream": False,
    }, separators=(",", ":")).encode("utf-8")
    status, response_body, sent, error = local_post(messages_path, body, 38.0)
    return selected_model, status, response_body, sent, error


verification_models = requested_models or [model]
with ThreadPoolExecutor(max_workers=len(verification_models)) as executor:
    verification_results = list(executor.map(verify_model, verification_models))
billable_request_sent = any(item[3] for item in verification_results)
failed_result = next(
    (item for item in verification_results
     if item[4] is not None or not isinstance(item[1], int) or not (200 <= item[1] < 300)),
    None,
)
if failed_result is None:
    request_layer = make_layer("passed", "request_ok", request_started, http_status=200,
                               detail=f"Verified model routes: {len(verification_results)}.")
    direct_layer = make_layer("skipped", "direct_skipped",
                              detail="Direct control is unnecessary after successful Bridge requests.")
    emit("ok", f"Bridge 已通过 {len(verification_results)} 次 max_tokens=1 的真实请求验证全部指定路由。",
         "无需修改出口配置；保留当前设置并按需复测。", ok=True)

model, request_status, request_response_body, request_was_sent, request_error = failed_result
effective_upstream = resolve_upstream_base(model)
upstream = upstream_endpoint(effective_upstream)
upstream_base_url = upstream["sanitized"] if upstream is not None else None


def safe_error_type(body):
    try:
        value = json.loads(body.decode("utf-8"))
        error = value.get("error") if isinstance(value, dict) else None
        error_type = error.get("type") if isinstance(error, dict) else None
        if isinstance(error_type, str) and re.fullmatch(r"[A-Za-z0-9_.-]{1,64}", error_type):
            return error_type
    except Exception:
        pass
    return None


error_type = safe_error_type(request_response_body)
error_detail = ("Bridge error type: " + error_type + ".") if error_type else None

if request_error == "timeout":
    request_layer = make_layer("failed", "upstream_timeout", request_started,
                               detail="The managed Bridge verification request timed out after at most 38 seconds.")
    request_failure = "upstream_timeout"
elif request_error is not None:
    request_layer = make_layer("failed", "upstream_unreachable", request_started,
                               detail="The local Bridge request became unreachable after dispatch.")
    request_failure = "upstream_unreachable"
elif request_status == 502:
    request_layer = make_layer("failed", "upstream_502", request_started,
                               http_status=request_status, detail=error_detail)
    request_failure = "upstream_502"
elif request_status == 401:
    request_layer = make_layer("failed", "upstream_401", request_started,
                               http_status=request_status, detail=error_detail)
    direct_layer = make_layer("skipped", "direct_skipped",
                              detail="HTTP 401 is an authentication response, not proof of proxy failure.")
    emit("upstream_401", "真实 Bridge 请求到达上游但返回 HTTP 401；这不是代理故障判据。",
         "核对当前 Provider、API Key 与上游账号权限；不要因 401 自动删除代理。")
elif isinstance(request_status, int) and 200 <= request_status < 300:
    request_layer = make_layer("passed", "request_ok", request_started,
                               http_status=request_status)
    direct_layer = make_layer("skipped", "direct_skipped",
                              detail="Direct control is unnecessary after a successful Bridge request.")
    emit("ok", "Bridge 已通过一次 max_tokens=1 的真实请求验证上游出口。",
         "无需修改出口配置；保留当前设置并按需复测。", ok=True)
else:
    request_layer = make_layer("failed", "upstream_http", request_started,
                               http_status=request_status, detail=error_detail)
    direct_layer = make_layer("skipped", "direct_skipped",
                              detail="Direct control only runs for HTTP 502 or timeout with a configured proxy.")
    emit("upstream_http", "真实 Bridge 请求返回非成功 HTTP 状态；未满足代理故障对照条件。",
         "按 HTTP 状态核对 Provider、模型与账号配置；不要自动修改系统网络。")

if request_failure in {"upstream_502", "upstream_timeout", "upstream_unreachable"}:
    candidates = discover_bridge_candidates(upstream)

if outbound_proxy_configured and request_failure in {"upstream_502", "upstream_timeout"}:
    direct_started = time.monotonic()
    direct_candidate = next(
        (item for item in candidates if item.get("address") == "direct"),
        None,
    )
    if direct_candidate is None:
        direct_layer = make_layer("failed", "direct_failed", direct_started,
                                  detail="Candidate discovery returned no direct origin control.")
    elif direct_candidate["upstream"]["state"] == "passed":
        direct_layer = make_layer(
            "passed", "direct_reachable", direct_started,
            http_status=direct_candidate["upstream"].get("httpStatus"),
            detail="The already-bounded direct candidate reached the configured upstream base URL with a non-billable GET; its semantics differ from the model request, so it cannot isolate the proxy.",
        )
    else:
        direct_layer = make_layer(
            "failed", "direct_failed", direct_started,
            detail="The already-bounded direct candidate could not reach the configured upstream base URL.",
        )
else:
    direct_layer = make_layer("skipped", "direct_skipped")

if request_failure == "upstream_timeout":
    emit("upstream_timeout", "真实 Bridge 请求超时；直连 origin 对照与模型请求语义不同，不能把故障单点归因到 outbound proxy。",
         "只读复核上游服务、DNS/TLS 与代理链路后再决定最小修复。")
if request_failure == "upstream_502":
    emit("upstream_502", "真实 Bridge 请求返回 HTTP 502；直连 origin 对照与模型请求语义不同，不能把故障单点归因到 outbound proxy。",
         "核对上游可用性和 Bridge 出口配置；不要自动修改系统代理。")
emit("upstream_unreachable", "真实 Bridge 请求在本地传输阶段失败。",
     "先确认受管 Bridge 仍在监听，再重跑出口体检。")
"#;

const BRIDGE_EGRESS_CONNECTION_PYTHON: &str = r#"
import hashlib
import http.client
import json
import re
import socket
import subprocess
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit

PREFIX = "work.bridge_egress."
runtime_pid = None
runtime_source_sha256 = None
runtime_starttime = None


def layer(state, name, detail=None):
    value = {
        "state": state,
        "code": PREFIX + name,
        "durationMs": 0,
    }
    if detail:
        value["detail"] = detail
    return value


def emit(name, proxy, outbound_proxy_url=None):
    value = {"code": PREFIX + name, "proxy": proxy}
    if outbound_proxy_url:
        value["outboundProxyUrl"] = outbound_proxy_url
    if runtime_pid:
        value["runtimePid"] = runtime_pid
    if runtime_source_sha256:
        value["runtimeSourceSha256"] = runtime_source_sha256
    if runtime_starttime:
        value["runtimeStarttime"] = runtime_starttime
    print(json.dumps(value, ensure_ascii=True, separators=(",", ":")))
    raise SystemExit(0)


def safe_proxy(raw):
    try:
        parts = urlsplit(str(raw or ""))
        scheme = parts.scheme.lower()
        host = parts.hostname
        if scheme not in {"http", "https", "socks5", "socks5h"} or not host:
            return None
        if any(ord(character) < 33 for character in host):
            return None
        port = parts.port or {"http": 80, "https": 443, "socks5": 1080, "socks5h": 1080}[scheme]
        if port < 1 or port > 65535:
            return None
        display = "[" + host + "]" if ":" in host and not host.startswith("[") else host
        return host, port, urlunsplit((scheme, display + ":" + str(port), "", "", ""))
    except Exception:
        return None


def valid_managed_identity(identity):
    if not isinstance(identity, dict):
        return False
    source_sha256 = identity.get("sourceSha256")
    build_id = identity.get("buildId")
    return (
        identity.get("schemaVersion") == 1
        and identity.get("component") == "bridge"
        and identity.get("managed") is True
        and isinstance(identity.get("runtimeId"), str)
        and re.fullmatch(r"[A-Za-z0-9._-]+", identity.get("runtimeId") or "") is not None
        and isinstance(identity.get("version"), str)
        and bool(identity.get("version").strip())
        and isinstance(build_id, str)
        and re.fullmatch(r"[0-9A-Fa-f]{16}", build_id or "") is not None
        and isinstance(identity.get("sourcePath"), str)
        and bool(identity.get("sourcePath").strip())
        and isinstance(source_sha256, str)
        and re.fullmatch(r"[0-9A-Fa-f]{64}", source_sha256 or "") is not None
        and build_id.casefold() == source_sha256[:16].casefold()
        and isinstance(identity.get("pid"), int)
        and not isinstance(identity.get("pid"), bool)
        and identity.get("pid") > 0
        and isinstance(identity.get("capabilities"), list)
        and "health" in identity.get("capabilities")
    )


def valid_current_bridge_identity(identity):
    try:
        current_proxy = (Path.home() / ".local" / "share" / "csa" / "runtime" /
                         "bridge" / "current" / "proxy.py")
        source_path = Path(identity["sourcePath"])
        source_hash = hashlib.sha256(current_proxy.read_bytes()).hexdigest()
        identity_pid = int(identity["pid"])
        sockets = subprocess.run(
            ["ss", "-H", "-ltnp", "sport = :9876"],
            capture_output=True, text=True, timeout=0.8, check=False,
        )
        listener_pids = set(int(value) for value in re.findall(r"pid=(\d+)", sockets.stdout))
        argv = (Path("/proc") / str(identity_pid) / "cmdline").read_bytes().split(b"\0")
        argv_proxy_paths = []
        for item in argv:
            try:
                value = item.decode("utf-8")
                if value.endswith("proxy.py"):
                    argv_proxy_paths.append(Path(value).resolve())
            except Exception:
                pass
        return (
            current_proxy.resolve() == source_path.resolve()
            and source_hash.casefold() == str(identity["sourceSha256"]).casefold()
            and listener_pids == {identity_pid}
            and current_proxy.resolve() in argv_proxy_paths
        )
    except Exception:
        return False


def process_starttime(pid):
    try:
        value = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
        tail = value[value.rfind(")") + 2:].split()
        starttime = tail[19]
        return starttime if re.fullmatch(r"\d{1,32}", starttime or "") else None
    except Exception:
        return None


try:
    connection = http.client.HTTPConnection("127.0.0.1", 9876, timeout=1.5)
    connection.request("GET", "/health", headers={"Connection": "close"})
    response = connection.getresponse()
    body = response.read(1024 * 1024 + 1)
    connection.close()
except Exception:
    emit("health_unreachable", layer("failed", "health_unreachable",
                                     "Bridge /health is unreachable inside WSL."))

if response.status != 200:
    emit("health_unreachable", layer("failed", "health_unreachable",
                                     "Bridge /health did not return HTTP 200."))
try:
    health = json.loads(body.decode("utf-8"))
except Exception:
    health = None
identity = health.get("runtime_identity") if isinstance(health, dict) else None
if not valid_managed_identity(identity) or not valid_current_bridge_identity(identity):
    emit("health_identity_invalid", layer("failed", "health_identity_invalid",
                                          "Bridge runtime identity is invalid."))
identity_starttime = process_starttime(identity["pid"])
if identity_starttime is None:
    emit("health_identity_invalid", layer("failed", "health_identity_invalid",
                                          "Bridge runtime context is incomplete."))
runtime_pid = identity["pid"]
runtime_source_sha256 = str(identity["sourceSha256"]).lower()
runtime_starttime = identity_starttime
if health.get("outbound_proxy_configured") is not True:
    emit("proxy_not_configured", layer("skipped", "proxy_not_configured",
                                       "Bridge has no configured outbound proxy."))

endpoint = safe_proxy(health.get("outbound_proxy_url"))
if endpoint is None:
    emit("proxy_dead", layer("failed", "proxy_dead",
                             "Configured outbound proxy endpoint is invalid."))
host, port, outbound_proxy_url = endpoint
try:
    connection = socket.create_connection((host, port), timeout=1.5)
    connection.close()
except Exception:
    emit("proxy_dead", layer("failed", "proxy_dead",
                             "Configured outbound proxy did not accept a WSL TCP connection."),
         outbound_proxy_url)
emit("proxy_ok", layer("passed", "proxy_ok",
                       "Configured outbound proxy accepted a WSL TCP connection."),
     outbound_proxy_url)
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BridgeEgressLayerState {
    Passed,
    Failed,
    Skipped,
}

impl BridgeEgressLayerState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEgressLayer {
    pub state: BridgeEgressLayerState,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEgressConnectionReport {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_proxy_url: Option<String>,
    pub proxy: BridgeEgressLayer,
    #[serde(default, skip_serializing)]
    pub(crate) probe_distro: Option<String>,
    #[serde(default, skip_serializing)]
    pub(crate) runtime_pid: Option<u32>,
    #[serde(default, skip_serializing)]
    pub(crate) runtime_source_sha256: Option<String>,
    #[serde(default, skip_serializing)]
    pub(crate) runtime_starttime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEgressCandidate {
    pub address: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_name: Option<String>,
    pub tcp: BridgeEgressLayer,
    pub upstream: BridgeEgressLayer,
    pub recommended: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeEgressReport {
    pub operation: String,
    pub ok: bool,
    pub code: String,
    pub conclusion: String,
    pub billable_request_sent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub outbound_proxy_configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_proxy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,
    pub health: BridgeEgressLayer,
    pub proxy: BridgeEgressLayer,
    pub models: BridgeEgressLayer,
    pub request: BridgeEgressLayer,
    pub direct: BridgeEgressLayer,
    #[serde(default)]
    pub candidates: Vec<BridgeEgressCandidate>,
    pub suggested_action: String,
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing)]
    pub(crate) probe_distro: Option<String>,
    #[serde(default, skip_serializing)]
    pub(crate) runtime_pid: Option<u32>,
    #[serde(default, skip_serializing)]
    pub(crate) runtime_source_sha256: Option<String>,
    #[serde(default, skip_serializing)]
    pub(crate) runtime_starttime: Option<String>,
}

fn report_url_is_sanitized(value: &str, proxy_only: bool) -> bool {
    if value.is_empty()
        || value.contains('@')
        || value.contains('?')
        || value.contains('#')
        || value.chars().any(char::is_control)
    {
        return false;
    }
    let Some((scheme, endpoint)) = value.split_once("://") else {
        return false;
    };
    let scheme_valid = if proxy_only {
        matches!(scheme, "http" | "https" | "socks5" | "socks5h")
    } else {
        matches!(scheme, "http" | "https")
    };
    if !scheme_valid || endpoint.is_empty() {
        return false;
    }
    !endpoint.contains('/')
        && (!proxy_only
            || endpoint.rsplit_once(':').is_some()
            || (endpoint.starts_with('[') && endpoint.contains("]:")))
}

fn text_is_bounded(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && !value.chars().any(char::is_control)
}

fn managed_context_is_safe(context: &ManagedBridgeProofContext) -> bool {
    !context.distro.trim().is_empty()
        && text_is_bounded(&context.distro, 128)
        && context.runtime_pid > 0
        && context.runtime_source_sha256.len() == 64
        && context
            .runtime_source_sha256
            .bytes()
            .all(|value| value.is_ascii_hexdigit())
        && !context.runtime_starttime.is_empty()
        && context.runtime_starttime.len() <= 32
        && context
            .runtime_starttime
            .bytes()
            .all(|value| value.is_ascii_digit())
}

fn model_id_is_safe(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    value.len() <= 256
        && first.is_ascii_alphanumeric()
        && characters.all(|value| {
            value.is_ascii_alphanumeric() || matches!(value, '.' | '_' | ':' | '/' | '-')
        })
}

fn validate_layer(layer: &BridgeEgressLayer) -> bool {
    layer.code.starts_with("work.bridge_egress.")
        && text_is_bounded(&layer.code, 96)
        && layer
            .detail
            .as_deref()
            .map(|value| text_is_bounded(value, 500))
            .unwrap_or(true)
}

fn validate_candidate(candidate: &BridgeEgressCandidate) -> bool {
    let address_valid =
        candidate.address == "direct" || report_url_is_sanitized(&candidate.address, true);
    let source_valid = matches!(
        candidate.source.as_str(),
        "windows_system_proxy" | "listening_port" | "direct"
    );
    let source_address_valid = (candidate.source == "direct") == (candidate.address == "direct");
    let process_valid = candidate
        .process_name
        .as_deref()
        .map(|value| text_is_bounded(value, 96))
        .unwrap_or(true);
    let tcp_valid = validate_layer(&candidate.tcp)
        && match candidate.tcp.state {
            BridgeEgressLayerState::Passed => {
                candidate.tcp.code == "work.bridge_egress.candidate.tcp_ok"
            }
            BridgeEgressLayerState::Failed => {
                candidate.tcp.code == "work.bridge_egress.candidate.unreachable"
            }
            BridgeEgressLayerState::Skipped => false,
        };
    let upstream_valid = validate_layer(&candidate.upstream)
        && match candidate.upstream.state {
            BridgeEgressLayerState::Passed => {
                candidate.upstream.code == "work.bridge_egress.candidate.upstream_ok"
            }
            BridgeEgressLayerState::Failed => {
                candidate.upstream.code == "work.bridge_egress.candidate.upstream_unreachable"
            }
            BridgeEgressLayerState::Skipped => {
                candidate.upstream.code == "work.bridge_egress.candidate.upstream_skipped"
                    && candidate.tcp.state == BridgeEgressLayerState::Failed
            }
        };
    address_valid
        && source_valid
        && source_address_valid
        && process_valid
        && tcp_valid
        && upstream_valid
        && text_is_bounded(&candidate.reason, 500)
        && (!candidate.recommended || candidate.upstream.state == BridgeEgressLayerState::Passed)
}

fn validate_report(report: &BridgeEgressReport, confirm_billable: bool) -> Result<(), String> {
    let layers = [
        &report.health,
        &report.proxy,
        &report.models,
        &report.request,
        &report.direct,
    ];
    let core_valid = report.operation == "bridge_egress"
        && report.code.starts_with("work.bridge_egress.")
        && text_is_bounded(&report.code, 96)
        && text_is_bounded(&report.conclusion, 1_000)
        && text_is_bounded(&report.suggested_action, 1_000)
        && report
            .model
            .as_deref()
            .map(model_id_is_safe)
            .unwrap_or(true)
        && report.warnings.len() <= 8
        && report
            .warnings
            .iter()
            .all(|value| text_is_bounded(value, 500))
        && report.candidates.len() <= 12
        && report.candidates.iter().all(validate_candidate)
        && report
            .candidates
            .iter()
            .filter(|candidate| candidate.recommended)
            .count()
            <= 1
        && report
            .candidates
            .iter()
            .enumerate()
            .all(|(index, candidate)| {
                !report.candidates[..index]
                    .iter()
                    .any(|other| other.address.eq_ignore_ascii_case(&candidate.address))
            })
        && layers.into_iter().all(validate_layer);
    let urls_valid = report
        .outbound_proxy_url
        .as_deref()
        .map(|value| report_url_is_sanitized(value, true))
        .unwrap_or(true)
        && report
            .upstream_base_url
            .as_deref()
            .map(|value| report_url_is_sanitized(value, false))
            .unwrap_or(true);
    let billing_valid = (confirm_billable || !report.billable_request_sent)
        && (!report.ok
            || (report.billable_request_sent
                && report.request.state == BridgeEgressLayerState::Passed));
    let managed_context_valid = report.health.state != BridgeEgressLayerState::Passed
        || report_managed_context(report)
            .as_ref()
            .is_some_and(managed_context_is_safe);
    let early_proxy_failure_valid = report.proxy.state != BridgeEgressLayerState::Failed
        || (!report.billable_request_sent
            && report.models.state == BridgeEgressLayerState::Skipped
            && report.request.state == BridgeEgressLayerState::Skipped
            && report.direct.state == BridgeEgressLayerState::Skipped);
    let failure_candidates_valid = !matches!(
        report.code.as_str(),
        "work.bridge_egress.proxy_dead"
            | "work.bridge_egress.upstream_502"
            | "work.bridge_egress.upstream_timeout"
            | "work.bridge_egress.upstream_unreachable"
    ) || !report.candidates.is_empty();
    if core_valid
        && urls_valid
        && billing_valid
        && managed_context_valid
        && early_proxy_failure_valid
        && failure_candidates_valid
    {
        Ok(())
    } else {
        Err("work.bridge_egress.probe_contract".into())
    }
}

fn report_managed_context(report: &BridgeEgressReport) -> Option<ManagedBridgeProofContext> {
    Some(ManagedBridgeProofContext {
        distro: report.probe_distro.clone()?,
        runtime_pid: report.runtime_pid?,
        runtime_source_sha256: report.runtime_source_sha256.clone()?,
        runtime_starttime: report.runtime_starttime.clone()?,
    })
}

fn connection_managed_context(
    report: &BridgeEgressConnectionReport,
) -> Option<ManagedBridgeProofContext> {
    Some(ManagedBridgeProofContext {
        distro: report.probe_distro.clone()?,
        runtime_pid: report.runtime_pid?,
        runtime_source_sha256: report.runtime_source_sha256.clone()?,
        runtime_starttime: report.runtime_starttime.clone()?,
    })
}

fn report_proves_exact_candidate(report: &BridgeEgressReport, candidate_url: &str) -> bool {
    let requested_address = if candidate_url.trim().is_empty() {
        "direct"
    } else {
        candidate_url.trim()
    };
    report.candidates.iter().any(|candidate| {
        candidate.address.eq_ignore_ascii_case(requested_address)
            && candidate.tcp.state == BridgeEgressLayerState::Passed
            && candidate.upstream.state == BridgeEgressLayerState::Passed
    })
}

fn record_candidate_proofs(report: &BridgeEgressReport) {
    let addresses = report
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.tcp.state == BridgeEgressLayerState::Passed
                && candidate.upstream.state == BridgeEgressLayerState::Passed
        })
        .map(|candidate| candidate.address.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let upstream_origin = report.upstream_base_url.clone();
    let managed_context = report_managed_context(report);
    if let Ok(mut cache) = candidate_proofs().lock() {
        if addresses.is_empty() || upstream_origin.is_none() || managed_context.is_none() {
            *cache = CandidateProofCache::default();
        } else {
            cache.recorded_at = Some(Instant::now());
            cache.addresses = addresses;
            cache.upstream_origin = upstream_origin;
            cache.managed_context = managed_context;
        }
    }
}

fn take_recent_candidate_proof(candidate_url: &str) -> Option<CandidateProofEvidence> {
    let requested = if candidate_url.trim().is_empty() {
        "direct".to_string()
    } else {
        candidate_url.trim().to_ascii_lowercase()
    };
    let Ok(mut cache) = candidate_proofs().lock() else {
        return None;
    };
    let fresh = cache
        .recorded_at
        .is_some_and(|recorded| recorded.elapsed() <= CANDIDATE_PROOF_MAX_AGE);
    if !fresh {
        *cache = CandidateProofCache::default();
        return None;
    }
    let Some(index) = cache
        .addresses
        .iter()
        .position(|address| address == &requested)
    else {
        return None;
    };
    cache.addresses.remove(index);
    let evidence = CandidateProofEvidence {
        upstream_origin: cache.upstream_origin.clone()?,
        managed_context: cache.managed_context.clone()?,
    };
    // A proof is single-use. Clear sibling addresses too so a second write
    // always requires a newly displayed WORK report.
    *cache = CandidateProofCache::default();
    Some(evidence)
}

pub(super) fn consume_recent_candidate_proof(candidate_url: &str) -> bool {
    let Some(evidence) = take_recent_candidate_proof(candidate_url) else {
        return false;
    };
    // Re-run the authoritative non-billable probe immediately before the
    // write. This re-resolves the current full upstream base URL from live
    // Bridge config, rebinds PID/hash/starttime/distro, and re-tests the exact
    // candidate. No config revision is trusted as a proxy for content.
    let Ok(current_report) = run_bridge_egress_probe(false) else {
        return false;
    };
    if let Ok(mut cache) = candidate_proofs().lock() {
        *cache = CandidateProofCache::default();
    }
    let Some(current_context) = report_managed_context(&current_report) else {
        return false;
    };
    if current_context != evidence.managed_context
        || current_report.upstream_base_url.as_deref() != Some(&evidence.upstream_origin)
    {
        return false;
    }
    report_proves_exact_candidate(&current_report, candidate_url)
}

fn probe_exit_error(code: Option<i32>) -> &'static str {
    if matches!(code, Some(124 | 137)) {
        "work.bridge_egress.watchdog_timeout"
    } else {
        "work.bridge_egress.probe_failed"
    }
}

fn guest_watchdog_argument(remaining: Duration, timeout_code: &str) -> Result<String, String> {
    let guest = remaining
        .checked_sub(GUEST_PROCESS_RESERVE)
        .filter(|value| *value >= Duration::from_millis(200))
        .ok_or_else(|| timeout_code.to_string())?;
    Ok(format!("{}.{:03}s", guest.as_secs(), guest.subsec_millis()))
}

/// Runs the authoritative five-layer Bridge egress probe inside the selected WSL distro.
/// This is the only production entry point used by both the Tauri command and smoke.
pub fn run_bridge_egress_probe(confirm_billable: bool) -> Result<BridgeEgressReport, String> {
    run_bridge_egress_probe_for_models(confirm_billable, &[])
}

pub(crate) fn run_bridge_egress_probe_for_models(
    confirm_billable: bool,
    requested_models: &[&str],
) -> Result<BridgeEgressReport, String> {
    if requested_models.is_empty() {
        // The ordinary WORK probe chooses its established preferred/fallback
        // model. Transactional aggregate verification passes exactly 3 aliases.
    } else if requested_models.len() > 3
        || requested_models
            .iter()
            .any(|model| !model_id_is_safe(model))
        || requested_models
            .iter()
            .enumerate()
            .any(|(index, model)| requested_models[..index].contains(model))
    {
        return Err("work.bridge_egress.model_invalid".into());
    }
    let started = Instant::now();
    let discovery_budget = std::cmp::min(DISTRO_DISCOVERY_BUDGET, BRIDGE_EGRESS_WATCHDOG);
    let distros = discover_distros_with_timeout(discovery_budget)
        .map_err(|_| "work.bridge_egress.wsl_unavailable".to_string())?;
    let distro = preferred_distro(&distros)
        .ok_or_else(|| "work.bridge_egress.wsl_distro_missing".to_string())?;
    let remaining = BRIDGE_EGRESS_WATCHDOG
        .checked_sub(started.elapsed())
        .filter(|value| *value >= Duration::from_millis(100))
        .ok_or_else(|| "work.bridge_egress.watchdog_timeout".to_string())?;
    let guest_watchdog = guest_watchdog_argument(remaining, "work.bridge_egress.watchdog_timeout")?;

    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(&distro)
        .arg("--")
        .arg("timeout")
        .arg("--signal=TERM")
        .arg("--kill-after=1s")
        .arg(guest_watchdog)
        .arg("python3")
        .arg("-")
        .arg(if confirm_billable { "1" } else { "0" });
    for model in requested_models {
        command.arg(model);
    }
    let output = command_output_with_stdin_timeout(
        command,
        BRIDGE_EGRESS_PROBE_PYTHON.as_bytes(),
        remaining,
        "Bridge egress WSL probe",
    )
    .map_err(|_| {
        if started.elapsed() >= BRIDGE_EGRESS_WATCHDOG {
            "work.bridge_egress.watchdog_timeout".to_string()
        } else {
            "work.bridge_egress.transport_failed".to_string()
        }
    })?;
    if !output.status.success() {
        return Err(probe_exit_error(output.status.code()).into());
    }
    let text = output_text(&output);
    let payload = text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| "work.bridge_egress.probe_invalid".to_string())?;
    let mut report: BridgeEgressReport = serde_json::from_str(payload)
        .map_err(|_| "work.bridge_egress.probe_invalid".to_string())?;
    report.probe_distro = Some(distro);
    validate_report(&report, confirm_billable)?;
    record_candidate_proofs(&report);
    Ok(report)
}

/// Reads only Bridge health and the configured outbound proxy TCP layer.
/// Key preflight attribution may use this path; it never discovers candidates,
/// reaches an upstream, sends a model request, or mutates configuration.
pub fn run_bridge_egress_connection_probe() -> Result<BridgeEgressConnectionReport, String> {
    let started = Instant::now();
    let discovery_budget =
        std::cmp::min(DISTRO_DISCOVERY_BUDGET, BRIDGE_EGRESS_CONNECTION_WATCHDOG);
    let distros = discover_distros_with_timeout(discovery_budget)
        .map_err(|_| "work.bridge_egress.connection_wsl_unavailable".to_string())?;
    let distro = preferred_distro(&distros)
        .ok_or_else(|| "work.bridge_egress.connection_wsl_distro_missing".to_string())?;
    let remaining = BRIDGE_EGRESS_CONNECTION_WATCHDOG
        .checked_sub(started.elapsed())
        .filter(|value| *value >= Duration::from_millis(100))
        .ok_or_else(|| "work.bridge_egress.connection_timeout".to_string())?;
    let guest_watchdog =
        guest_watchdog_argument(remaining, "work.bridge_egress.connection_timeout")?;

    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(&distro)
        .arg("--")
        .arg("timeout")
        .arg("--signal=TERM")
        .arg("--kill-after=1s")
        .arg(guest_watchdog)
        .arg("python3")
        .arg("-");
    let output = command_output_with_stdin_timeout(
        command,
        BRIDGE_EGRESS_CONNECTION_PYTHON.as_bytes(),
        remaining,
        "Bridge egress connection attribution probe",
    )
    .map_err(|_| {
        if started.elapsed() >= BRIDGE_EGRESS_CONNECTION_WATCHDOG {
            "work.bridge_egress.connection_timeout".to_string()
        } else {
            "work.bridge_egress.connection_transport_failed".to_string()
        }
    })?;
    if !output.status.success() {
        return Err(if matches!(output.status.code(), Some(124 | 137)) {
            "work.bridge_egress.connection_timeout".to_string()
        } else {
            "work.bridge_egress.connection_probe_failed".to_string()
        });
    }
    let text = output_text(&output);
    let payload = text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| "work.bridge_egress.connection_probe_invalid".to_string())?;
    let mut report: BridgeEgressConnectionReport = serde_json::from_str(payload)
        .map_err(|_| "work.bridge_egress.connection_probe_invalid".to_string())?;
    report.probe_distro = Some(distro);
    let valid = report.code.starts_with("work.bridge_egress.")
        && text_is_bounded(&report.code, 96)
        && validate_layer(&report.proxy)
        && report
            .outbound_proxy_url
            .as_deref()
            .map(|value| report_url_is_sanitized(value, true))
            .unwrap_or(true);
    let context_required = !matches!(
        report.code.as_str(),
        "work.bridge_egress.health_unreachable" | "work.bridge_egress.health_identity_invalid"
    );
    let context_valid = !context_required
        || connection_managed_context(&report)
            .as_ref()
            .is_some_and(managed_context_is_safe);
    if !valid || !context_valid {
        return Err("work.bridge_egress.connection_probe_contract".into());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(state: BridgeEgressLayerState, name: &str) -> BridgeEgressLayer {
        BridgeEgressLayer {
            state,
            code: format!("work.bridge_egress.{name}"),
            http_status: None,
            duration_ms: 0,
            detail: None,
        }
    }

    fn reachable_candidate(
        address: &str,
        source: &str,
        recommended: bool,
    ) -> BridgeEgressCandidate {
        BridgeEgressCandidate {
            address: address.into(),
            source: source.into(),
            process_name: Some("Hiddify".into()),
            tcp: layer(BridgeEgressLayerState::Passed, "candidate.tcp_ok"),
            upstream: layer(BridgeEgressLayerState::Passed, "candidate.upstream_ok"),
            recommended,
            reason: "Windows system proxy reaches current upstream".into(),
        }
    }

    fn proxy_dead_report() -> BridgeEgressReport {
        BridgeEgressReport {
            operation: "bridge_egress".into(),
            ok: false,
            code: "work.bridge_egress.proxy_dead".into(),
            conclusion: "proxy unavailable".into(),
            billable_request_sent: false,
            model: None,
            outbound_proxy_configured: true,
            outbound_proxy_url: Some("http://127.0.0.1:10808".into()),
            upstream_base_url: Some("https://api.deepseek.com".into()),
            health: layer(BridgeEgressLayerState::Passed, "health_ok"),
            proxy: layer(BridgeEgressLayerState::Failed, "proxy_dead"),
            models: layer(BridgeEgressLayerState::Skipped, "models_skipped"),
            request: layer(BridgeEgressLayerState::Skipped, "request_skipped"),
            direct: layer(BridgeEgressLayerState::Skipped, "direct_skipped"),
            candidates: vec![reachable_candidate(
                "http://127.0.0.1:12334",
                "windows_system_proxy",
                true,
            )],
            suggested_action: "replace or clear the Bridge proxy".into(),
            warnings: Vec::new(),
            probe_distro: Some("Ubuntu-24.04".into()),
            runtime_pid: Some(4242),
            runtime_source_sha256: Some("a".repeat(64)),
            runtime_starttime: Some("123456".into()),
        }
    }

    #[test]
    fn proxy_dead_report_is_fixed_five_layer_and_non_billable() {
        let report = proxy_dead_report();
        validate_report(&report, true).unwrap();
        assert_eq!(report.health.state, BridgeEgressLayerState::Passed);
        assert_eq!(report.proxy.state, BridgeEgressLayerState::Failed);
        assert_eq!(report.models.state, BridgeEgressLayerState::Skipped);
        assert_eq!(report.request.state, BridgeEgressLayerState::Skipped);
        assert_eq!(report.direct.state, BridgeEgressLayerState::Skipped);
        assert!(!report.billable_request_sent);
    }

    #[test]
    fn bridge_egress_detects_dead_proxy() {
        // Freeze the production glue as well as the DTO contract. This test
        // must fail if either the dead-endpoint check or candidate discovery
        // is removed from the executable WSL probe.
        assert!(BRIDGE_EGRESS_PROBE_PYTHON.contains(
            "proxy_layer, proxy_failure_conclusion, proxy_failure_action = probe_configured_proxy(endpoint)"
        ));
        let dead_branch = BRIDGE_EGRESS_PROBE_PYTHON
            .find("if proxy_failure_conclusion is not None:")
            .unwrap();
        let candidate_discovery = BRIDGE_EGRESS_PROBE_PYTHON[dead_branch..]
            .find("candidates = discover_bridge_candidates(upstream)")
            .unwrap();
        let dead_emit = BRIDGE_EGRESS_PROBE_PYTHON[dead_branch..]
            .find("emit(\"proxy_dead\", proxy_failure_conclusion, proxy_failure_action)")
            .unwrap();
        assert!(candidate_discovery < dead_emit);
        assert!(BRIDGE_EGRESS_PROBE_PYTHON.contains("make_layer(\"failed\", \"proxy_dead\""));
        assert!(BRIDGE_EGRESS_PROBE_PYTHON.contains("candidate.unreachable"));
        assert!(BRIDGE_EGRESS_PROBE_PYTHON.contains("candidate.upstream_unreachable"));

        // Fault-inject a genuinely closed port into the executable production
        // Python functions.  The fixture trims only the real Bridge top-level
        // runner, replaces Windows discovery with an empty read-only result,
        // and serves a loopback upstream so the unconditional direct candidate
        // can prove itself.  Together with the glue assertions above, this
        // catches both dead-endpoint classification and discovery regressions.
        let library = BRIDGE_EGRESS_PROBE_PYTHON
            .split_once("\nhealth_started = time.monotonic()")
            .map(|(value, _)| value)
            .expect("probe library marker");
        let fixture = format!(
            r#"{library}
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread

class QuietServer(ThreadingHTTPServer):
    def handle_error(self, request, client_address):
        pass

class Handler(BaseHTTPRequestHandler):
    paths = []
    def do_GET(self):
        self.__class__.paths.append(self.path)
        self.send_response(401)
        self.end_headers()
    def log_message(self, format, *args):
        pass

server = QuietServer(("127.0.0.1", 0), Handler)
Thread(target=server.serve_forever, daemon=True).start()
dead_socket = socket.socket()
dead_socket.bind(("127.0.0.1", 0))
dead_port = dead_socket.getsockname()[1]
read_windows_proxy_candidates = lambda: (None, [])
try:
    dead_endpoint = proxy_endpoint(f"http://127.0.0.1:{{dead_port}}")
    dead_layer, conclusion, action = probe_configured_proxy(dead_endpoint)
    upstream = upstream_endpoint(f"http://127.0.0.1:{{server.server_port}}/configured/base")
    fixture_candidates = discover_bridge_candidates(upstream)
    print(json.dumps({{
        "proxy": dead_layer,
        "conclusion": conclusion,
        "action": action,
        "candidates": fixture_candidates,
        "paths": Handler.paths,
    }}, separators=(",", ":")))
finally:
    dead_socket.close()
    server.shutdown()
    server.server_close()
"#
        );
        let distros = discover_distros_with_timeout(Duration::from_millis(1_500))
            .expect("WSL distro discovery for dead-proxy fixture");
        let distro = preferred_distro(&distros).expect("preferred WSL distro");
        let mut command = background_command("wsl.exe");
        command
            .arg("--distribution")
            .arg(&distro)
            .arg("--")
            .arg("python3")
            .arg("-");
        let output = command_output_with_stdin_timeout(
            command,
            fixture.as_bytes(),
            Duration::from_secs(12),
            "dead proxy fault-injection fixture",
        )
        .expect("dead-proxy fixture execution");
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            output_text(&output),
            String::from_utf8_lossy(&output.stderr)
        );
        let payload: serde_json::Value =
            serde_json::from_str(output_text(&output).trim()).expect("fixture JSON");
        assert_eq!(payload["proxy"]["state"], "failed");
        assert_eq!(payload["proxy"]["code"], "work.bridge_egress.proxy_dead");
        let injected_candidates = payload["candidates"].as_array().expect("candidate list");
        assert!(!injected_candidates.is_empty());
        assert_eq!(injected_candidates[0]["address"], "direct");
        assert_eq!(injected_candidates[0]["upstream"]["state"], "passed");
        assert_eq!(injected_candidates[0]["recommended"], true);
        assert_eq!(payload["paths"][0], "/configured/base");

        // Execute the complete production top-level with only its external
        // health/config sources replaced. A dead configured endpoint must emit
        // before /v1/models, preserve the five-layer short circuit, include a
        // real candidate result, and deserialize through the Rust contract.
        let (_, top_level) = BRIDGE_EGRESS_PROBE_PYTHON
            .split_once("\nhealth_started = time.monotonic()")
            .expect("probe top-level marker");
        let top_level = top_level.replace(
            "bridge_environment = read_bridge_environment(identity[\"pid\"])\nbridge_config = read_bridge_config(bridge_environment)",
            r#"bridge_environment = {}
bridge_config = {
    "default_backend": "custom",
    "force_model": "byok-model-0001",
    "custom_base_url": f"http://127.0.0.1:{server.server_port}",
    "custom_upstream_mode": "openai",
    "model_aliases": [{"id": "byok-model-0001", "backend": "custom", "model": "fixture-model"}],
}"#,
        );
        let prelude = r#"
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread

class FullHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(401)
        self.end_headers()
    def log_message(self, format, *args):
        pass

server = ThreadingHTTPServer(("127.0.0.1", 0), FullHandler)
Thread(target=server.serve_forever, daemon=True).start()
dead_socket = socket.socket()
dead_socket.bind(("127.0.0.1", 0))
dead_port = dead_socket.getsockname()[1]

def fixture_local_get(path, timeout):
    if path != "/health":
        raise RuntimeError("dead proxy top-level must short-circuit before models")
    value = {
        "runtime_identity": {
            "schemaVersion": 1,
            "component": "bridge",
            "managed": True,
            "runtimeId": "bridge-fixture",
            "version": "0.1.6",
            "buildId": "a" * 16,
            "sourcePath": "/fixture/proxy.py",
            "sourceSha256": "a" * 64,
            "pid": 4242,
            "capabilities": ["health"],
        },
        "outbound_proxy_configured": True,
        "outbound_proxy_url": f"http://127.0.0.1:{dead_port}",
        "proxy_auth_mode": "optional",
        "config_revision": "",
    }
    return 200, json.dumps(value).encode("utf-8")

local_get = fixture_local_get
valid_managed_identity = lambda identity: True
valid_current_bridge_identity = lambda identity: True
process_starttime = lambda pid: "123456"
read_windows_proxy_candidates = lambda: (None, [])
"#;
        let full_fixture =
            format!("{library}{prelude}\nhealth_started = time.monotonic(){top_level}");
        let mut full_command = background_command("wsl.exe");
        full_command
            .arg("--distribution")
            .arg(&distro)
            .arg("--")
            .arg("python3")
            .arg("-")
            .arg("0");
        let full_output = command_output_with_stdin_timeout(
            full_command,
            full_fixture.as_bytes(),
            Duration::from_secs(15),
            "dead proxy full top-level fixture",
        )
        .expect("full dead-proxy fixture execution");
        assert!(
            full_output.status.success(),
            "{}",
            output_text(&full_output)
        );
        let full_text = output_text(&full_output);
        let full_payload = full_text
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('{'))
            .expect("full fixture JSON");
        let mut full_report: BridgeEgressReport =
            serde_json::from_str(full_payload).expect("full fixture report");
        full_report.probe_distro = Some(distro.clone());
        validate_report(&full_report, false).expect("full dead-proxy report contract");
        assert_eq!(full_report.code, "work.bridge_egress.proxy_dead");
        assert_eq!(full_report.models.state, BridgeEgressLayerState::Skipped);
        assert_eq!(full_report.request.state, BridgeEgressLayerState::Skipped);
        assert!(!full_report.billable_request_sent);
        assert!(!full_report.candidates.is_empty());

        let mut missing_candidates = proxy_dead_report();
        missing_candidates.candidates.clear();
        assert_eq!(
            validate_report(&missing_candidates, true).unwrap_err(),
            "work.bridge_egress.probe_contract"
        );

        let report = proxy_dead_report();
        validate_report(&report, true).unwrap();
        assert_eq!(report.code, "work.bridge_egress.proxy_dead");
        assert!(!report.candidates.is_empty());
        let recommendation = report
            .candidates
            .iter()
            .find(|candidate| candidate.recommended)
            .unwrap();
        assert_eq!(recommendation.address, "http://127.0.0.1:12334");
        assert_eq!(
            recommendation.tcp.code,
            "work.bridge_egress.candidate.tcp_ok"
        );
        assert_eq!(
            recommendation.upstream.code,
            "work.bridge_egress.candidate.upstream_ok"
        );
    }

    #[test]
    fn report_contract_rejects_secrets_and_unconfirmed_billing() {
        let mut report = proxy_dead_report();
        report.outbound_proxy_url = Some("http://user:secret@127.0.0.1:10808?token=x".into());
        assert_eq!(
            validate_report(&report, true).unwrap_err(),
            "work.bridge_egress.probe_contract"
        );

        let mut report = proxy_dead_report();
        report.proxy = layer(BridgeEgressLayerState::Passed, "proxy_ok");
        report.models = layer(BridgeEgressLayerState::Passed, "models_ok");
        report.request = layer(BridgeEgressLayerState::Failed, "upstream_502");
        report.direct = layer(BridgeEgressLayerState::Passed, "direct_reachable");
        report.billable_request_sent = true;
        assert!(validate_report(&report, true).is_ok());
        assert!(validate_report(&report, false).is_err());

        let mut report = proxy_dead_report();
        report.conclusion = "unsafe\nline".into();
        assert!(validate_report(&report, true).is_err());

        let mut report = proxy_dead_report();
        report.outbound_proxy_url = Some("ftp://127.0.0.1:10808".into());
        assert!(validate_report(&report, true).is_err());

        let mut report = proxy_dead_report();
        report.proxy = layer(BridgeEgressLayerState::Passed, "proxy_ok");
        report.models = layer(BridgeEgressLayerState::Passed, "models_ok");
        report.model = Some("unsafe model".into());
        assert!(validate_report(&report, true).is_err());
    }

    #[test]
    fn apply_candidate_requires_a_recent_single_use_proof() {
        let report = proxy_dead_report();
        assert!(report_proves_exact_candidate(
            &report,
            "HTTP://127.0.0.1:12334"
        ));
        assert!(!report_proves_exact_candidate(&report, ""));
        let mut failed_candidate = report.clone();
        failed_candidate.candidates[0].upstream = layer(
            BridgeEgressLayerState::Failed,
            "candidate.upstream_unreachable",
        );
        assert!(!report_proves_exact_candidate(
            &failed_candidate,
            "http://127.0.0.1:12334"
        ));
        record_candidate_proofs(&report);
        let evidence = take_recent_candidate_proof("http://127.0.0.1:12334").unwrap();
        assert_eq!(evidence.upstream_origin, "https://api.deepseek.com");
        assert_eq!(evidence.managed_context.runtime_pid, 4242);
        assert!(take_recent_candidate_proof("http://127.0.0.1:12334").is_none());

        record_candidate_proofs(&report);
        assert!(take_recent_candidate_proof("http://proxy.example.com:8080").is_none());
    }

    #[test]
    fn source_contract_guards_billing_transport_identity_and_secrets() {
        let source = BRIDGE_EGRESS_PROBE_PYTHON;
        assert_eq!(
            source
                .matches("connection.request(\n            \"POST\"")
                .count(),
            1
        );
        let confirmation = source.find("if not confirm_billable:").unwrap();
        let post = source
            .find("connection.request(\n            \"POST\"")
            .unwrap();
        assert!(confirmation < post);
        assert!(source.contains("\"max_tokens\": 1"));
        assert!(source.contains("local_get(\"/health\", 2.0)"));
        assert!(source.contains("bounded_timeout(1.5)"));
        assert!(source.contains("local_post(messages_path, body, 38.0)"));
        assert!(source.contains("verification_models = requested_models or [model]"));
        assert!(source.contains("PROBE_DEADLINE = time.monotonic() + 56.0"));
        assert!(source.contains("Path(\"/proc\") / str(pid) / \"environ\""));
        assert!(source.contains("valid_current_bridge_identity"));
        assert!(source.contains("current_proxy.resolve() == source_path.resolve()"));
        assert!(source.contains("listener_pids == {identity_pid}"));
        assert!(source.contains("[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}"));
        assert!(source.contains("/ \"config.json\""));
        assert!(source.contains("urlunsplit((scheme, netloc, \"\", \"\", \"\"))"));
        assert!(source.contains("\"curl\", \"--disable\""));
        assert!(source.contains("command.extend([\"--noproxy\", \"\", \"--proxy\", address])"));
        assert!(source.contains("\"--config\", \"-\""));
        assert!(source.contains("input=curl_config"));
        assert!(!source.contains("command.append(target)"));
        assert!(source.contains("\"path\": path"));
        assert!(source.contains("current configured upstream base-URL path"));
        assert!(source.contains("merged = {}"));
        assert!(
            source.contains("HTTP 401 is an authentication response, not proof of proxy failure")
        );
        assert!(!source.contains("/api/config"));
        assert!(!source.contains("bash -lc"));
        assert!(!source.contains("/mnt/"));
        assert!(!source.contains("Authorization"));
        assert!(!source.contains("x-api-key"));
        let token_read = source.find("proxy_auth_token = effective_text").unwrap();
        let models_recheck = source[token_read..]
            .find("if (not valid_current_bridge_identity(identity)")
            .map(|index| index + token_read)
            .unwrap();
        let models_request = source.find("local_get(models_path, 4.0)").unwrap();
        let billable_recheck = source[models_request..]
            .find("if (not valid_current_bridge_identity(identity)")
            .map(|index| index + models_request)
            .unwrap();
        let billable_request = source[models_request..]
            .find("local_post(messages_path, body, 38.0)")
            .map(|index| index + models_request)
            .unwrap();
        assert!(token_read < models_recheck && models_recheck < models_request);
        assert!(models_request < billable_recheck && billable_recheck < billable_request);
        assert_eq!(BRIDGE_EGRESS_WATCHDOG, Duration::from_secs(64));
        assert_eq!(BRIDGE_EGRESS_CONNECTION_WATCHDOG, Duration::from_secs(10));

        let connection_source = BRIDGE_EGRESS_CONNECTION_PYTHON;
        assert!(connection_source.contains("GET\", \"/health"));
        assert!(connection_source.contains("socket.create_connection"));
        assert!(connection_source.contains("valid_managed_identity"));
        assert!(connection_source.contains("valid_current_bridge_identity"));
        assert!(connection_source.contains("listener_pids == {identity_pid}"));
        assert!(!connection_source.contains("Get-NetTCPConnection"));
        assert!(!connection_source.contains("powershell.exe"));
        assert!(!connection_source.contains("/v1/messages"));
        assert!(!connection_source.contains("/api/config"));

        let rust_source = include_str!("bridge_egress.rs");
        assert!(rust_source.contains(".arg(\"timeout\")"));
        assert!(rust_source.contains(".arg(\"--signal=TERM\")"));
        assert!(rust_source.contains(".arg(\"--kill-after=1s\")"));
        assert!(rust_source.contains("guest_watchdog_argument(remaining"));
        assert_eq!(
            guest_watchdog_argument(Duration::from_secs(10), "timeout").unwrap(),
            "7.000s"
        );
        let production_source = &rust_source[..rust_source.find("#[cfg(test)]").unwrap()];
        assert!(!production_source.contains("bash -lc"));
    }

    #[test]
    fn guest_watchdog_exit_is_classified_as_a_timeout() {
        assert_eq!(
            probe_exit_error(Some(124)),
            "work.bridge_egress.watchdog_timeout"
        );
        assert_eq!(
            probe_exit_error(Some(137)),
            "work.bridge_egress.watchdog_timeout"
        );
        assert_eq!(probe_exit_error(Some(1)), "work.bridge_egress.probe_failed");
    }
}
