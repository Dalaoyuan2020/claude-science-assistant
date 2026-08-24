use super::{
    background_command, command_output_with_stdin_timeout, discover_distros_with_timeout,
    output_text, preferred_distro,
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

const BRIDGE_EGRESS_WATCHDOG: Duration = Duration::from_secs(64);
const DISTRO_DISCOVERY_BUDGET: Duration = Duration::from_millis(1_500);

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
from pathlib import Path
from urllib.parse import quote, urlsplit, urlunsplit

PREFIX = "work.bridge_egress."
PREFERRED_MODEL = "claude-haiku-4-5-20251001"
PROBE_DEADLINE = time.monotonic() + 62.0
MAX_LOCAL_BODY = 1024 * 1024
confirm_billable = len(sys.argv) == 2 and sys.argv[1] == "1"


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
        "suggestedAction": suggested_action,
        "warnings": warnings[:8],
    }
    if model:
        report["model"] = model
    if outbound_proxy_url:
        report["outboundProxyUrl"] = outbound_proxy_url
    if upstream_base_url:
        report["upstreamBaseUrl"] = upstream_base_url
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
        if scheme not in {"http", "https"} or not host:
            return None
        port = parts.port
        if port is not None and (port < 1 or port > 65535):
            return None
        netloc = display_host(host) + ((":" + str(port)) if port is not None else "")
        path = parts.path or ""
        # Keep the full path only inside the WSL probe for the direct control.
        # The DTO exposes origin only because custom providers sometimes embed
        # credentials in a base-URL path.
        sanitized = urlunsplit((scheme, netloc, "", "", ""))
        return {
            "scheme": scheme,
            "host": host,
            "port": port or (443 if scheme == "https" else 80),
            "path": path or "/",
            "sanitized": sanitized,
        }
    except Exception:
        return None


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
health_layer = make_layer("passed", "health_ok", health_started, http_status=health_status)
outbound_proxy_configured = health.get("outbound_proxy_configured") is True

if outbound_proxy_configured:
    endpoint = proxy_endpoint(health.get("outbound_proxy_url"))
    if endpoint is not None:
        outbound_proxy_url = endpoint["sanitized"]
    proxy_started = time.monotonic()
    if endpoint is None:
        proxy_layer = make_layer("failed", "proxy_dead", proxy_started,
                                 detail="Configured outbound proxy endpoint is invalid after redaction.")
        emit("proxy_dead", "Bridge 配置了无法安全解析的 outbound proxy；已短路，未发送真实模型请求。",
             "检查 Bridge 自己的 outbound_proxy_url，并改为 WSL 内实际可达的代理地址或置空。")
    try:
        connection = socket.create_connection(
            (endpoint["host"], endpoint["port"]),
            timeout=bounded_timeout(1.5),
        )
        connection.close()
        proxy_layer = make_layer("passed", "proxy_ok", proxy_started,
                                 detail="Configured proxy accepted a WSL TCP connection.")
    except Exception:
        proxy_layer = make_layer("failed", "proxy_dead", proxy_started,
                                 detail="Configured proxy did not accept a WSL TCP connection within 1.5 seconds.")
        emit("proxy_dead", "Bridge 配置的 outbound proxy 在 WSL 内未监听或不可达；已短路，未发送真实模型请求。",
             "将 outbound_proxy_url 置空，或替换为 WSL 内实际监听且可达的代理地址。")
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

model = PREFERRED_MODEL if PREFERRED_MODEL in model_ids else model_ids[0]
models_layer = make_layer("passed", "models_ok", models_started,
                          http_status=models_status,
                          detail=f"Usable model count: {len(model_ids)}.")


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
request_body = json.dumps({
    "model": model,
    "max_tokens": 1,
    "messages": [{"role": "user", "content": "Reply with one short word."}],
    "stream": False,
}, separators=(",", ":")).encode("utf-8")
request_started = time.monotonic()
request_status, request_response_body, request_was_sent, request_error = local_post(
    messages_path, request_body, 45.0
)
billable_request_sent = request_was_sent


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
                               detail="The single local Bridge request timed out after at most 45 seconds.")
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


def direct_get(endpoint, timeout):
    connection = None
    try:
        if endpoint["scheme"] == "https":
            connection = http.client.HTTPSConnection(
                endpoint["host"], endpoint["port"],
                timeout=bounded_timeout(timeout), context=ssl.create_default_context(),
            )
        else:
            connection = http.client.HTTPConnection(
                endpoint["host"], endpoint["port"], timeout=bounded_timeout(timeout)
            )
        connection.request("GET", endpoint["path"], headers={
            "Connection": "close",
            "User-Agent": "CSA-Bridge-Egress-Control/1",
        })
        response = connection.getresponse()
        response.read(64 * 1024)
        return response.status, None
    except (TimeoutError, socket.timeout):
        return None, "timeout"
    except Exception:
        return None, "unreachable"
    finally:
        if connection is not None:
            connection.close()


if outbound_proxy_configured and request_failure in {"upstream_502", "upstream_timeout"}:
    direct_started = time.monotonic()
    if upstream is None:
        direct_layer = make_layer("failed", "direct_failed", direct_started,
                                  detail="No safe upstream base URL was available for the direct control.")
    else:
        direct_status, direct_error = direct_get(upstream, 8.0)
        if isinstance(direct_status, int):
            direct_layer = make_layer("passed", "direct_reachable", direct_started,
                                      http_status=direct_status,
                                      detail="Direct no-key/no-proxy control reached the upstream HTTP service.")
            emit("proxy_dead", "Bridge 经 outbound proxy 的请求失败，但 WSL 不带 Key 直连上游可达；故障定位到 Bridge outbound proxy。",
                 "将 outbound_proxy_url 置空，或替换为 WSL 内实际可用的代理地址。")
        else:
            detail = ("Direct no-key/no-proxy control timed out." if direct_error == "timeout"
                      else "Direct no-key/no-proxy control could not reach the upstream service.")
            direct_layer = make_layer("failed", "direct_failed", direct_started, detail=detail)
else:
    direct_layer = make_layer("skipped", "direct_skipped")

if request_failure == "upstream_timeout":
    emit("upstream_timeout", "真实 Bridge 请求超时，且直连对照未证明是 outbound proxy 单点故障。",
         "只读复核上游服务、DNS/TLS 与代理链路后再决定最小修复。")
if request_failure == "upstream_502":
    emit("upstream_502", "真实 Bridge 请求返回 HTTP 502，且直连对照未证明是 outbound proxy 单点故障。",
         "核对上游可用性和 Bridge 出口配置；不要自动修改系统代理。")
emit("upstream_unreachable", "真实 Bridge 请求在本地传输阶段失败。",
     "先确认受管 Bridge 仍在监听，再重跑出口体检。")
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
    pub suggested_action: String,
    pub warnings: Vec<String>,
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
    let early_proxy_failure_valid = report.proxy.state != BridgeEgressLayerState::Failed
        || (!report.billable_request_sent
            && report.models.state == BridgeEgressLayerState::Skipped
            && report.request.state == BridgeEgressLayerState::Skipped
            && report.direct.state == BridgeEgressLayerState::Skipped);
    if core_valid && urls_valid && billing_valid && early_proxy_failure_valid {
        Ok(())
    } else {
        Err("work.bridge_egress.probe_contract".into())
    }
}

fn probe_exit_error(code: Option<i32>) -> &'static str {
    if matches!(code, Some(124 | 137)) {
        "work.bridge_egress.watchdog_timeout"
    } else {
        "work.bridge_egress.probe_failed"
    }
}

/// Runs the authoritative five-layer Bridge egress probe inside the selected WSL distro.
/// This is the only production entry point used by both the Tauri command and smoke.
pub fn run_bridge_egress_probe(confirm_billable: bool) -> Result<BridgeEgressReport, String> {
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

    let mut command = background_command("wsl.exe");
    command
        .arg("--distribution")
        .arg(&distro)
        .arg("--")
        .arg("timeout")
        .arg("--signal=TERM")
        .arg("--kill-after=1s")
        .arg("62s")
        .arg("python3")
        .arg("-")
        .arg(if confirm_billable { "1" } else { "0" });
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
    let report: BridgeEgressReport = serde_json::from_str(payload)
        .map_err(|_| "work.bridge_egress.probe_invalid".to_string())?;
    validate_report(&report, confirm_billable)?;
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
            upstream_base_url: None,
            health: layer(BridgeEgressLayerState::Passed, "health_ok"),
            proxy: layer(BridgeEgressLayerState::Failed, "proxy_dead"),
            models: layer(BridgeEgressLayerState::Skipped, "models_skipped"),
            request: layer(BridgeEgressLayerState::Skipped, "request_skipped"),
            direct: layer(BridgeEgressLayerState::Skipped, "direct_skipped"),
            suggested_action: "replace or clear the Bridge proxy".into(),
            warnings: Vec::new(),
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
        assert!(source.contains("messages_path, request_body, 45.0"));
        assert!(source.contains("Path(\"/proc\") / str(pid) / \"environ\""));
        assert!(source.contains("valid_current_bridge_identity"));
        assert!(source.contains("current_proxy.resolve() == source_path.resolve()"));
        assert!(source.contains("listener_pids == {identity_pid}"));
        assert!(source.contains("[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}"));
        assert!(source.contains("/ \"config.json\""));
        assert!(source.contains("urlunsplit((scheme, netloc, \"\", \"\", \"\"))"));
        assert!(
            source.contains("HTTP 401 is an authentication response, not proof of proxy failure")
        );
        assert!(!source.contains("/api/config"));
        assert!(!source.contains("bash -lc"));
        assert!(!source.contains("/mnt/"));
        assert!(!source.contains("Authorization"));
        assert!(!source.contains("x-api-key"));
        assert_eq!(BRIDGE_EGRESS_WATCHDOG, Duration::from_secs(64));

        let rust_source = include_str!("bridge_egress.rs");
        assert!(rust_source.contains(".arg(\"timeout\")"));
        assert!(rust_source.contains(".arg(\"--signal=TERM\")"));
        assert!(rust_source.contains(".arg(\"--kill-after=1s\")"));
        assert!(rust_source.contains(".arg(\"62s\")"));
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
