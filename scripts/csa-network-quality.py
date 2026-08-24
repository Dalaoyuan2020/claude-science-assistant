#!/usr/bin/env python3
"""Redacted proxy and Claude Science sandbox egress diagnostics.

The default probe is local-only: it reads a process environment, reports only
proxy variable names and credential-free endpoints, and checks loopback proxy
TCP listeners.  ``--deep`` additionally sends a no-auth, no-billing canary
request through Claude Science's canonical analysis SOCKS forwarder.
"""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import json
import os
import re
import select
import shutil
import socket
import stat
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, replace
from pathlib import Path, PurePosixPath
from typing import Mapping, Optional
from urllib.parse import urlsplit, urlunsplit


PROXY_VARIABLES = (
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
)
NETWORK_REPORT_SCHEMA_VERSION = 3
HEALTHY_PROXY_STATES = {"direct", "reachable"}
DEFAULT_CANARY_URL = "https://pypi.org/simple/pip/"
DEFAULT_CACHE_MAX_AGE_SECONDS = 15 * 60
DEFAULT_EXPECTED_SANDBOX_FORWARDERS = 3
SANDBOX_PROBE_IDENTITY = "analysis-socks5h-pypi-head-v2"
TRANSIENT_DAEMON_EGRESS_STATES = {"daemon_busy", "daemon_mount_io_busy"}
# The three built-in role forwarders are spawned together during daemon start.
# A later replacement can make PID ordering assign the wrong allowlist to a
# role, so fail closed when their start times are no longer one startup burst.
SANDBOX_ROLE_START_TICK_WINDOW = 500
SOCAT_EXECUTABLES = {"/usr/bin/socat", "/usr/bin/socat1", "/bin/socat", "/bin/socat1"}


@dataclass(frozen=True)
class ProxyEndpoint:
    scheme: str
    host: str
    port: int

    @property
    def display(self) -> str:
        host = f"[{self.host}]" if ":" in self.host else self.host
        return f"{self.scheme}://{host}:{self.port}"

    @property
    def loopback(self) -> bool:
        if self.host.casefold() == "localhost":
            return True
        try:
            return ipaddress.ip_address(self.host).is_loopback
        except ValueError:
            return False


@dataclass(frozen=True)
class SandboxForwarderProcess:
    pid: int
    start_ticks: int
    socket_directory: str
    transport: str
    port: int
    socket_path: str
    socket_device: int
    socket_inode: int


@dataclass(frozen=True)
class SandboxForwarder:
    role: str
    socket_directory: str
    first_pid: int
    http_port: Optional[int]
    socks_port: Optional[int]
    http_socket_path: Optional[str]
    socks_socket_path: Optional[str]
    http_socket_device: Optional[int]
    socks_socket_device: Optional[int]
    http_socket_inode: Optional[int]
    socks_socket_inode: Optional[int]
    member_pids: tuple[int, ...]
    member_start_ticks: tuple[int, ...]
    http_process_count: int
    socks_process_count: int

    @property
    def complete(self) -> bool:
        return (
            self.http_process_count == 1
            and self.socks_process_count == 1
            and self.http_port is not None
            and self.socks_port is not None
            and self.http_socket_path is not None
            and self.socks_socket_path is not None
            and self.http_socket_device is not None
            and self.socks_socket_device is not None
            and bool(self.http_socket_inode)
            and bool(self.socks_socket_inode)
        )


@dataclass(frozen=True)
class ProcessRuntimeSnapshot:
    state: str
    wait_channel: str
    io_blocked: bool
    mount_io_blocked: bool


def process_runtime_snapshot(pid: int) -> ProcessRuntimeSnapshot:
    """Read a redacted Linux scheduler snapshot for the daemon main thread."""
    state = "unknown"
    wait_channel = "unknown"
    try:
        payload = Path(f"/proc/{pid}/stat").read_text(
            encoding="utf-8", errors="replace"
        )
        suffix = payload[payload.rfind(")") + 2 :].split()
        if suffix and re.fullmatch(r"[A-Z]", suffix[0]):
            state = suffix[0]
    except OSError:
        pass
    try:
        candidate = Path(f"/proc/{pid}/wchan").read_text(
            encoding="utf-8", errors="replace"
        ).strip()
        if re.fullmatch(r"[A-Za-z0-9_.-]{1,80}", candidate):
            wait_channel = candidate
    except OSError:
        pass

    io_blocked = state == "D"
    normalized_wait = wait_channel.casefold()
    mount_io_blocked = io_blocked and any(
        marker in normalized_wait
        for marker in ("p9_", "v9fs", "fuse", "virtiofs", "virtio_fs")
    )
    return ProcessRuntimeSnapshot(
        state=state,
        wait_channel=wait_channel,
        io_blocked=io_blocked,
        mount_io_blocked=mount_io_blocked,
    )


def parse_proxy_endpoint(value: str) -> Optional[ProxyEndpoint]:
    value = value.strip()
    if not value:
        return None
    candidate = value if "://" in value else f"http://{value}"
    try:
        parsed = urlsplit(candidate)
        host = parsed.hostname
        if not host:
            return None
        scheme = (parsed.scheme or "http").casefold()
        default_port = {
            "http": 80,
            "https": 443,
            "socks": 1080,
            "socks4": 1080,
            "socks4a": 1080,
            "socks5": 1080,
            "socks5h": 1080,
        }.get(scheme)
        port = parsed.port or default_port
        if port is None or not (1 <= port <= 65535):
            return None
        return ProxyEndpoint(scheme=scheme, host=host, port=port)
    except (TypeError, ValueError):
        return None


def tcp_reachable(endpoint: ProxyEndpoint, timeout: float = 0.35) -> bool:
    try:
        with socket.create_connection((endpoint.host, endpoint.port), timeout=timeout):
            return True
    except OSError:
        return False


def inspect_proxy_environment(environment: Mapping[str, str]) -> dict:
    configured_names = []
    invalid_names = []
    endpoints_by_key = {}
    for name in PROXY_VARIABLES:
        value = environment.get(name, "")
        if not value.strip():
            continue
        configured_names.append(name)
        endpoint = parse_proxy_endpoint(value)
        if endpoint is None:
            invalid_names.append(name)
        else:
            endpoints_by_key[(endpoint.scheme, endpoint.host.casefold(), endpoint.port)] = endpoint

    endpoints = sorted(endpoints_by_key.values(), key=lambda item: item.display)
    report = {
        "proxy_state": "direct",
        "proxy_reachable": None,
        "proxy_endpoints": [item.display for item in endpoints],
        "proxy_variable_names": configured_names,
        "proxy_conflict": False,
    }
    if not configured_names:
        return report
    if invalid_names:
        report["proxy_state"] = "invalid"
        report["proxy_reachable"] = False
        report["proxy_conflict"] = bool(endpoints)
        return report

    reachability = [tcp_reachable(item) for item in endpoints]
    # Different proxy endpoints are ambiguous even when every TCP listener is
    # currently reachable.  Libraries disagree about upper/lower-case proxy
    # precedence, so preserving such an environment would make the probe and
    # the daemon potentially use different routes.
    if len(endpoints) > 1:
        report["proxy_state"] = "conflict"
        report["proxy_reachable"] = False
        report["proxy_conflict"] = True
        return report
    if all(reachability):
        report["proxy_state"] = "reachable"
        report["proxy_reachable"] = True
    elif any(reachability):
        report["proxy_state"] = "conflict"
        report["proxy_reachable"] = False
        report["proxy_conflict"] = True
    else:
        report["proxy_state"] = "unreachable"
        report["proxy_reachable"] = False
    return report


def process_environment(pid: int) -> tuple[Optional[dict], str]:
    path = Path(f"/proc/{pid}/environ")
    try:
        payload = path.read_bytes()
    except FileNotFoundError:
        return None, "not_running"
    except OSError:
        return None, "unknown"

    environment = {}
    for entry in payload.split(b"\0"):
        if b"=" not in entry:
            continue
        raw_name, raw_value = entry.split(b"=", 1)
        name = raw_name.decode("utf-8", errors="replace")
        if name in PROXY_VARIABLES:
            environment[name] = raw_value.decode("utf-8", errors="replace")
    return environment, "ok"


def process_parent_pid(pid: int) -> Optional[int]:
    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8", errors="replace")
        suffix = stat[stat.rfind(")") + 2 :].split()
        return int(suffix[1])
    except (OSError, ValueError, IndexError):
        return None


def process_start_ticks(pid: int) -> Optional[int]:
    """Return Linux /proc start ticks, which disambiguate PID reuse."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8", errors="replace")
        suffix = stat[stat.rfind(")") + 2 :].split()
        return int(suffix[19])
    except (OSError, ValueError, IndexError):
        return None


def process_owner_home(pid: int) -> Optional[PurePosixPath]:
    """Resolve the daemon owner's canonical passwd home, independent of HOME."""
    try:
        status_lines = Path(f"/proc/{pid}/status").read_text(
            encoding="utf-8", errors="replace"
        ).splitlines()
        uid_line = next(line for line in status_lines if line.startswith("Uid:"))
        uid = int(uid_line.split()[1])
        import pwd

        home = PurePosixPath(pwd.getpwuid(uid).pw_dir)
    except (ImportError, OSError, KeyError, StopIteration, ValueError, IndexError):
        return None
    return home if home.is_absolute() else None


def parse_sandbox_forwarder_command(
    pid: int,
    start_ticks: int,
    arguments: list[str],
    claude_pid: Optional[int] = None,
    sandbox_root: Optional[PurePosixPath] = None,
) -> Optional[SandboxForwarderProcess]:
    """Parse one direct socat child without trusting arbitrary TCP targets."""
    if not arguments or PurePosixPath(arguments[0]).name != "socat":
        return None
    listeners = [
        argument for argument in arguments[1:] if argument.startswith("UNIX-LISTEN:")
    ]
    targets = [
        argument
        for argument in arguments[1:]
        if re.match(r"^TCP(?:4|6)?:", argument)
    ]
    if len(listeners) != 1 or len(targets) != 1:
        return None
    listener = listeners[0]
    target = targets[0]

    socket_path = listener.split(":", 1)[1].split(",", 1)[0]
    parsed_socket_path = PurePosixPath(socket_path)
    if not parsed_socket_path.is_absolute():
        return None
    socket_name = parsed_socket_path.name
    if socket_name == "http.sock":
        transport = "http"
    elif socket_name == "socks.sock":
        transport = "socks"
    else:
        return None
    socket_directory = parsed_socket_path.parent
    if claude_pid is not None or sandbox_root is not None:
        if claude_pid is None or sandbox_root is None:
            return None
        expected_name = re.compile(rf"^sock-{claude_pid}-[A-Za-z0-9_-]+$")
        if (
            socket_directory.parent != sandbox_root
            or not expected_name.fullmatch(socket_directory.name)
        ):
            return None
    match = re.match(
        r"^TCP(?:4|6)?:(?:localhost|127\.0\.0\.1|\[?::1\]?):(\d+)(?:,|$)",
        target,
        flags=re.IGNORECASE,
    )
    if not match:
        return None
    port = int(match.group(1))
    if not (1 <= port <= 65535):
        return None
    return SandboxForwarderProcess(
        pid=pid,
        start_ticks=start_ticks,
        socket_directory=str(socket_directory),
        transport=transport,
        port=port,
        socket_path=str(parsed_socket_path),
        socket_device=0,
        socket_inode=0,
    )


def group_sandbox_forwarders(
    processes: list[SandboxForwarderProcess],
) -> list[SandboxForwarder]:
    """Group HTTP/SOCKS listeners and map daemon spawn order to known roles."""
    grouped: dict[str, list[SandboxForwarderProcess]] = {}
    for process in sorted(processes, key=lambda item: item.pid):
        grouped.setdefault(process.socket_directory, []).append(process)

    ordered_groups = sorted(
        grouped.items(), key=lambda item: min(process.pid for process in item[1])
    )
    known_roles = ("analysis", "operon", "byoc")
    forwarders = []
    for index, (socket_directory, members) in enumerate(ordered_groups):
        role = known_roles[index] if index < len(known_roles) else f"infer-{index - 2}"
        http_members = [item for item in members if item.transport == "http"]
        socks_members = [item for item in members if item.transport == "socks"]
        forwarders.append(
            SandboxForwarder(
                role=role,
                socket_directory=socket_directory,
                first_pid=min(item.pid for item in members),
                http_port=http_members[0].port if http_members else None,
                socks_port=socks_members[0].port if socks_members else None,
                http_socket_path=http_members[0].socket_path if http_members else None,
                socks_socket_path=socks_members[0].socket_path if socks_members else None,
                http_socket_device=http_members[0].socket_device if http_members else None,
                socks_socket_device=socks_members[0].socket_device if socks_members else None,
                http_socket_inode=http_members[0].socket_inode if http_members else None,
                socks_socket_inode=socks_members[0].socket_inode if socks_members else None,
                member_pids=tuple(sorted(item.pid for item in members)),
                member_start_ticks=tuple(
                    item.start_ticks for item in sorted(members, key=lambda item: item.pid)
                ),
                http_process_count=len(http_members),
                socks_process_count=len(socks_members),
            )
        )
    return forwarders


def sandbox_forwarders(claude_pid: int) -> list[SandboxForwarder]:
    processes = []
    owner_home = process_owner_home(claude_pid)
    if owner_home is None:
        return []
    sandbox_root = owner_home / ".claude-science" / "sbx-bind-src"
    try:
        process_directories = list(Path("/proc").iterdir())
    except OSError:
        return []
    for process_dir in process_directories:
        if not process_dir.name.isdigit():
            continue
        pid = int(process_dir.name)
        parent_before = process_parent_pid(pid)
        start_ticks_before = process_start_ticks(pid)
        if parent_before != claude_pid or start_ticks_before is None:
            continue
        try:
            executable_before = os.readlink(process_dir / "exe")
            payload = (process_dir / "cmdline").read_bytes()
        except OSError:
            continue
        parent_after = process_parent_pid(pid)
        start_ticks_after = process_start_ticks(pid)
        try:
            executable_after = os.readlink(process_dir / "exe")
        except OSError:
            continue
        if (
            executable_before != executable_after
            or executable_after not in SOCAT_EXECUTABLES
            or parent_after != claude_pid
            or start_ticks_after != start_ticks_before
        ):
            continue
        arguments = [
            item.decode("utf-8", errors="replace")
            for item in payload.split(b"\0")
            if item
        ]
        parsed = parse_sandbox_forwarder_command(
            pid, start_ticks_before, arguments, claude_pid, sandbox_root
        )
        if parsed is None:
            continue
        try:
            socket_metadata = os.lstat(parsed.socket_path)
        except OSError:
            continue
        if not stat.S_ISSOCK(socket_metadata.st_mode):
            continue
        processes.append(
            replace(
                parsed,
                socket_device=socket_metadata.st_dev,
                socket_inode=socket_metadata.st_ino,
            )
        )
    return group_sandbox_forwarders(processes)


def sandbox_forwarder_fingerprint(forwarders: list[SandboxForwarder]) -> Optional[str]:
    if not forwarders:
        return None
    identity = "|".join(
        ":".join(
            (
                item.role,
                item.socket_directory,
                str(item.first_pid),
                ",".join(str(pid) for pid in item.member_pids),
                ",".join(str(ticks) for ticks in item.member_start_ticks),
                str(item.http_port or 0),
                str(item.socks_port or 0),
                item.http_socket_path or "-",
                item.socks_socket_path or "-",
                str(item.http_socket_device or 0),
                str(item.socks_socket_device or 0),
                str(item.http_socket_inode or 0),
                str(item.socks_socket_inode or 0),
                str(item.http_process_count),
                str(item.socks_process_count),
            )
        )
        for item in forwarders
    )
    return hashlib.sha256(identity.encode("utf-8")).hexdigest()[:16]


def socket_identity_matches(path: str, device: int, inode: int) -> bool:
    try:
        metadata = os.lstat(path)
    except OSError:
        return False
    return (
        stat.S_ISSOCK(metadata.st_mode)
        and metadata.st_dev == device
        and metadata.st_ino == inode
    )


class UnixSocketTcpAdapter:
    """Expose one Unix SOCKS socket on an ephemeral loopback TCP listener."""

    def __init__(self, unix_socket_path: str):
        self.unix_socket_path = unix_socket_path
        self.listener: Optional[socket.socket] = None
        self.client: Optional[socket.socket] = None
        self.upstream: Optional[socket.socket] = None
        self.thread: Optional[threading.Thread] = None
        self.stop_event = threading.Event()
        self.port = 0
        self.error: Optional[str] = None

    def __enter__(self) -> "UnixSocketTcpAdapter":
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(0.2)
        self.listener = listener
        self.port = int(listener.getsockname()[1])
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()
        return self

    def _serve(self) -> None:
        try:
            while not self.stop_event.is_set():
                try:
                    client, _address = self.listener.accept() if self.listener else (None, None)
                except socket.timeout:
                    continue
                if client is None:
                    return
                self.client = client
                break
            if self.client is None or self.stop_event.is_set():
                return

            upstream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            upstream.settimeout(3)
            upstream.connect(self.unix_socket_path)
            upstream.settimeout(None)
            self.upstream = upstream
            peers = {self.client: upstream, upstream: self.client}
            while peers and not self.stop_event.is_set():
                readable, _writable, _exceptional = select.select(
                    list(peers), [], [], 0.2
                )
                for source in readable:
                    target = peers[source]
                    data = source.recv(64 * 1024)
                    if data:
                        target.sendall(data)
                        continue
                    peers.pop(source, None)
                    try:
                        target.shutdown(socket.SHUT_WR)
                    except OSError:
                        pass
        except OSError as error:
            if not self.stop_event.is_set():
                self.error = type(error).__name__
        finally:
            self._close_socket(self.client)
            self._close_socket(self.upstream)

    @staticmethod
    def _close_socket(candidate: Optional[socket.socket]) -> None:
        if candidate is None:
            return
        try:
            candidate.close()
        except OSError:
            pass

    def __exit__(self, _exc_type, _exc_value, _traceback) -> None:
        self.stop_event.set()
        self._close_socket(self.listener)
        self._close_socket(self.client)
        self._close_socket(self.upstream)
        if self.thread is not None:
            self.thread.join(timeout=1)


def probe_socks_handshake(
    unix_socket_path: str, timeout: float = 2.0
) -> tuple[str, str, Optional[str]]:
    """Verify that the owned Unix socket reaches a live SOCKS5 server."""
    if not hasattr(socket, "AF_UNIX"):
        return "unavailable", "not_checked", "AfUnixUnavailable"
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(timeout)
    connected = False
    try:
        client.connect(unix_socket_path)
        connected = True
        client.sendall(b"\x05\x01\x00")
        response = b""
        while len(response) < 2:
            chunk = client.recv(2 - len(response))
            if not chunk:
                break
            response += chunk
        if response == b"\x05\x00":
            return "connected", "ok", None
        if len(response) == 2 and response[:1] == b"\x05":
            return "connected", "rejected", None
        return "connected", "invalid_reply", None
    except socket.timeout:
        return (
            "connected" if connected else "timeout",
            "timeout" if connected else "not_checked",
            "TimeoutError",
        )
    except OSError as error:
        return (
            "connected" if connected else "error",
            "error" if connected else "not_checked",
            type(error).__name__,
        )
    finally:
        try:
            client.close()
        except OSError:
            pass


def canary_target(
    url: str,
) -> tuple[Optional[str], bool, Optional[str], Optional[str]]:
    try:
        parsed = urlsplit(url)
        scheme = parsed.scheme.casefold()
        host = parsed.hostname.casefold() if parsed.hostname else None
        port = parsed.port
    except ValueError:
        return None, False, None, None
    valid = (
        scheme in {"http", "https"}
        and bool(host)
        and parsed.username is None
        and parsed.password is None
    )
    if not valid or host is None:
        return host, False, None, None

    default_port = 80 if scheme == "http" else 443
    display_host = f"[{host}]" if ":" in host else host
    authority = display_host if port in {None, default_port} else f"{display_host}:{port}"
    path = parsed.path or "/"
    # The human-readable identity intentionally omits query data, which may be
    # user supplied.  The fingerprint still binds the cache to the complete,
    # normalized target without disclosing it.
    safe_identity = urlunsplit((scheme, authority, path, "", ""))
    normalized = urlunsplit((scheme, authority, path, parsed.query, ""))
    fingerprint = hashlib.sha256(normalized.encode("utf-8")).hexdigest()
    return host, True, safe_identity, fingerprint


def probe_sandbox_egress(
    forwarders: list[SandboxForwarder],
    url: str,
    deep: bool,
    expected_forwarders: int = DEFAULT_EXPECTED_SANDBOX_FORWARDERS,
) -> dict:
    target, valid_target, canary_identity, canary_fingerprint = canary_target(url)
    complete_forwarders = [item for item in forwarders if item.complete]
    http_count = sum(item.http_process_count for item in forwarders)
    socks_count = sum(item.socks_process_count for item in forwarders)
    builtin_start_ticks = [
        ticks
        for item in forwarders[:expected_forwarders]
        for ticks in item.member_start_ticks
    ]
    roles_stable = (
        len(forwarders) >= expected_forwarders
        and len(builtin_start_ticks) == expected_forwarders * 2
        and max(builtin_start_ticks) - min(builtin_start_ticks)
        <= SANDBOX_ROLE_START_TICK_WINDOW
    )
    builtin_forwarders = forwarders[:expected_forwarders]
    if len(builtin_forwarders) < expected_forwarders or any(
        not item.complete for item in builtin_forwarders
    ):
        topology_state = "incomplete"
    elif not roles_stable:
        topology_state = "unstable"
    elif len(forwarders) == expected_forwarders:
        topology_state = "expected"
    else:
        topology_state = "extended"
    incomplete_extra_count = sum(
        1 for item in forwarders[expected_forwarders:] if not item.complete
    )
    result = {
        "deep_checked": bool(deep),
        "sandbox_forwarder_count": len(complete_forwarders),
        "sandbox_forwarder_group_count": len(forwarders),
        "sandbox_http_forwarder_count": http_count,
        "sandbox_socks_forwarder_count": socks_count,
        "sandbox_forwarder_expected_count": expected_forwarders,
        "sandbox_forwarder_topology_state": topology_state,
        "sandbox_forwarder_incomplete_extra_count": incomplete_extra_count,
        "sandbox_forwarder_fingerprint": sandbox_forwarder_fingerprint(forwarders),
        "sandbox_probe_identity": SANDBOX_PROBE_IDENTITY,
        "sandbox_probe_role": "analysis",
        "sandbox_probe_transport": "socks5h",
        "sandbox_unix_socket_state": "not_checked",
        "sandbox_socks_handshake_state": "not_checked",
        "sandbox_socks_handshake_error": None,
        "sandbox_egress_state": "not_checked",
        "sandbox_egress_failure_stage": "not_checked",
        "sandbox_egress_target": target,
        "sandbox_egress_canary_identity": canary_identity,
        "sandbox_egress_canary_fingerprint": canary_fingerprint,
        "sandbox_egress_http_status": None,
        "sandbox_egress_http_statuses": [],
        "sandbox_forwarder_probe_count": 0,
        "sandbox_forwarder_passed_count": 0,
        "sandbox_forwarder_failed_count": 0,
        "sandbox_egress_curl_exit_code": None,
    }
    if not deep:
        return result
    if not valid_target or url != DEFAULT_CANARY_URL:
        result["sandbox_egress_state"] = "invalid_target"
        result["sandbox_egress_failure_stage"] = "target"
        return result
    analysis = next((item for item in forwarders if item.role == "analysis"), None)
    if analysis is None or not analysis.complete or analysis.socks_port is None:
        result["sandbox_egress_state"] = (
            "topology_incomplete" if topology_state == "incomplete" else "unavailable"
        )
        result["sandbox_egress_failure_stage"] = "topology"
        return result
    unix_socket_state, handshake_state, handshake_error = probe_socks_handshake(
        analysis.socks_socket_path
    )
    result["sandbox_unix_socket_state"] = unix_socket_state
    result["sandbox_socks_handshake_state"] = handshake_state
    result["sandbox_socks_handshake_error"] = handshake_error
    handshake_identity_valid = socket_identity_matches(
        analysis.socks_socket_path,
        analysis.socks_socket_device,
        analysis.socks_socket_inode,
    )
    if unix_socket_state != "connected" or not handshake_identity_valid:
        result["sandbox_egress_state"] = "unix_socket_failed"
        result["sandbox_egress_failure_stage"] = "unix_socket"
        result["sandbox_forwarder_probe_count"] = 1
        result["sandbox_forwarder_failed_count"] = 1
        return result
    if handshake_state != "ok":
        result["sandbox_egress_state"] = "socks_handshake_failed"
        result["sandbox_egress_failure_stage"] = "socks_handshake"
        result["sandbox_forwarder_probe_count"] = 1
        result["sandbox_forwarder_failed_count"] = 1
        return result
    curl = shutil.which("curl")
    if not curl:
        result["sandbox_egress_state"] = "unavailable"
        result["sandbox_egress_failure_stage"] = "https_request"
        return result

    clean_environment = dict(os.environ)
    for name in (*PROXY_VARIABLES, "NO_PROXY", "no_proxy"):
        clean_environment.pop(name, None)
    status = 0
    process_passed = False
    adapter_error = "unavailable"
    try:
        with UnixSocketTcpAdapter(analysis.socks_socket_path) as adapter:
            command = [
                curl,
                "--disable",
                "--silent",
                "--show-error",
                "--output",
                os.devnull,
                "--write-out",
                "%{http_code}",
                "--noproxy",
                "",
                "--socks5-hostname",
                f"127.0.0.1:{adapter.port}",
                "--connect-timeout",
                "3",
                "--max-time",
                "8",
                "--retry",
                "0",
                "--user-agent",
                "CSA-Network-Quality/1",
                "--head",
                url,
            ]
            completed = subprocess.run(
                command,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=10,
                check=False,
                env=clean_environment,
            )
            try:
                status = int(completed.stdout.strip() or "0")
            except ValueError:
                status = 0
            result["sandbox_egress_curl_exit_code"] = completed.returncode
            process_passed = completed.returncode == 0 and 200 <= status < 300
        adapter_error = adapter.error
    except (OSError, subprocess.TimeoutExpired):
        pass
    passed = (
        process_passed
        and adapter_error is None
        and socket_identity_matches(
            analysis.socks_socket_path,
            analysis.socks_socket_device,
            analysis.socks_socket_inode,
        )
    )

    statuses = [status]
    passed_count = int(passed)
    failed_count = int(not passed)
    nonzero_statuses = [status for status in statuses if status]
    unique_statuses = sorted(set(nonzero_statuses))
    result["sandbox_egress_http_statuses"] = unique_statuses
    result["sandbox_forwarder_probe_count"] = 1
    result["sandbox_forwarder_passed_count"] = passed_count
    result["sandbox_forwarder_failed_count"] = failed_count

    if topology_state not in {"expected", "extended"}:
        result["sandbox_egress_state"] = f"topology_{topology_state}"
        result["sandbox_egress_failure_stage"] = "topology"
    elif passed:
        result["sandbox_egress_state"] = "ok"
        result["sandbox_egress_failure_stage"] = "none"
    elif unique_statuses == [502]:
        result["sandbox_egress_state"] = "http_502"
        result["sandbox_egress_failure_stage"] = "https_request"
    elif unique_statuses == [403]:
        result["sandbox_egress_state"] = "http_403"
        result["sandbox_egress_failure_stage"] = "https_request"
    elif len(unique_statuses) > 1:
        result["sandbox_egress_state"] = "mixed_http_failure"
        result["sandbox_egress_failure_stage"] = "https_request"
    else:
        result["sandbox_egress_state"] = "failed"
        result["sandbox_egress_failure_stage"] = "https_request"
    if len(unique_statuses) == 1:
        result["sandbox_egress_http_status"] = unique_statuses[0]
    return result


def build_report(
    pid: Optional[int],
    deep: bool,
    canary_url: str,
    expected_forwarders: int = DEFAULT_EXPECTED_SANDBOX_FORWARDERS,
) -> dict:
    runtime_before = ProcessRuntimeSnapshot("unknown", "unknown", False, False)
    if pid is None:
        proxy_report = inspect_proxy_environment(os.environ)
        forwarders = []
        start_ticks = None
    else:
        runtime_before = process_runtime_snapshot(pid)
        environment, process_state = process_environment(pid)
        if environment is None:
            proxy_report = {
                "proxy_state": process_state,
                "proxy_reachable": None,
                "proxy_endpoints": [],
                "proxy_variable_names": [],
                "proxy_conflict": False,
            }
        else:
            proxy_report = inspect_proxy_environment(environment)
        start_ticks = process_start_ticks(pid) if process_state == "ok" else None
        forwarders = (
            sandbox_forwarders(pid)
            if process_state == "ok" and start_ticks is not None
            else []
        )

    egress_report = probe_sandbox_egress(
        forwarders, canary_url, deep, expected_forwarders
    )
    runtime_after = (
        process_runtime_snapshot(pid)
        if pid is not None
        else ProcessRuntimeSnapshot("unknown", "unknown", False, False)
    )
    blocked_snapshot = next(
        (
            snapshot
            for snapshot in (runtime_before, runtime_after)
            if snapshot.mount_io_blocked
        ),
        next(
            (
                snapshot
                for snapshot in (runtime_before, runtime_after)
                if snapshot.io_blocked
            ),
            runtime_after,
        ),
    )
    egress_report.update(
        {
            "sandbox_probe_daemon_state": blocked_snapshot.state,
            "sandbox_probe_daemon_wait_channel": blocked_snapshot.wait_channel,
            "sandbox_probe_daemon_io_blocked": (
                runtime_before.io_blocked or runtime_after.io_blocked
            ),
            "sandbox_probe_daemon_mount_io_blocked": (
                runtime_before.mount_io_blocked or runtime_after.mount_io_blocked
            ),
        }
    )
    # A successful HTTP response does not make the daemon ready when either
    # runtime snapshot caught its event loop in uninterruptible I/O.  This is
    # especially important for the startup streak: two nominal HTTP results
    # must also be two stable daemon observations, otherwise a green cache can
    # hide an intermittent DrvFS/9P stall.
    if (
        deep
        and egress_report.get("sandbox_egress_failure_stage")
        in {"none", "unix_socket", "socks_handshake", "https_request"}
        and egress_report["sandbox_probe_daemon_io_blocked"]
    ):
        egress_report["sandbox_egress_state"] = (
            "daemon_mount_io_busy"
            if egress_report["sandbox_probe_daemon_mount_io_blocked"]
            else "daemon_busy"
        )
        egress_report["sandbox_egress_failure_stage"] = "daemon_event_loop"
    contract_stable_during_probe = None
    if deep:
        contract_stable_during_probe = False
        if pid is not None and start_ticks is not None:
            refreshed_start_ticks = process_start_ticks(pid)
            refreshed_forwarders = sandbox_forwarders(pid)
            contract_stable_during_probe = (
                refreshed_start_ticks == start_ticks
                and sandbox_forwarder_fingerprint(refreshed_forwarders)
                == sandbox_forwarder_fingerprint(forwarders)
            )
        if not contract_stable_during_probe:
            egress_report["sandbox_egress_state"] = "contract_changed"
            egress_report["sandbox_egress_failure_stage"] = "contract"
    egress_report["sandbox_contract_stable_during_probe"] = (
        contract_stable_during_probe
    )

    return {
        "schema_version": NETWORK_REPORT_SCHEMA_VERSION,
        "claude_pid": pid,
        "claude_start_ticks": start_ticks,
        "claude_process_state": runtime_after.state,
        "claude_wait_channel": runtime_after.wait_channel,
        "claude_io_blocked": runtime_after.io_blocked,
        "claude_mount_io_blocked": runtime_after.mount_io_blocked,
        **proxy_report,
        **egress_report,
        "deep_checked_at_unix": int(time.time()) if deep else None,
        "secrets_included": False,
    }


def deep_result_has_transient_daemon_block(report: Mapping[str, object]) -> bool:
    return bool(
        report.get("sandbox_egress_state") in TRANSIENT_DAEMON_EGRESS_STATES
        or report.get("sandbox_probe_daemon_io_blocked") is True
        or report.get("sandbox_probe_daemon_mount_io_blocked") is True
    )


def invalidate_cache(cache_file: Path) -> bool:
    try:
        cache_file.unlink()
        return True
    except FileNotFoundError:
        return True
    except OSError:
        return False


def merge_fresh_cache(report: dict, cache_file: Path, max_age_seconds: int) -> dict:
    report = dict(report)
    if report.get("claude_pid") is not None and (
        report.get("claude_io_blocked") is True
        or report.get("claude_process_state") in {None, "", "unknown", "D", "T", "t", "Z"}
    ):
        # A cached green result must never mask a daemon which cannot currently
        # service its own proxy listeners or whose live scheduler state cannot
        # be established.
        return report
    try:
        cached = json.loads(cache_file.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError):
        return report
    if (
        deep_result_has_transient_daemon_block(cached)
        and report.get("claude_pid") is not None
        and report.get("claude_io_blocked") is False
        and report.get("claude_process_state") in {"R", "S", "I"}
    ):
        # A D-state observation is evidence about one probe interval, not a
        # 15-minute property of a daemon which has since returned to a safe
        # scheduler state.  Do not combine the old p9_client_rpc snapshot with
        # the live do_epoll_wait state or keep presenting a recovered service
        # as broken.
        invalidate_cache(cache_file)
        return report
    checked_at = cached.get("deep_checked_at_unix")
    if not isinstance(checked_at, int):
        return report
    age = int(time.time()) - checked_at
    if age < 0 or age > max_age_seconds:
        return report
    identity_fields = (
        "schema_version",
        "claude_pid",
        "claude_start_ticks",
        "proxy_state",
        "proxy_endpoints",
        "sandbox_forwarder_count",
        "sandbox_forwarder_group_count",
        "sandbox_http_forwarder_count",
        "sandbox_socks_forwarder_count",
        "sandbox_forwarder_expected_count",
        "sandbox_forwarder_topology_state",
        "sandbox_forwarder_incomplete_extra_count",
        "sandbox_forwarder_fingerprint",
        "sandbox_probe_identity",
        "sandbox_probe_role",
        "sandbox_probe_transport",
        "sandbox_egress_canary_fingerprint",
    )
    if any(cached.get(name) != report.get(name) for name in identity_fields):
        return report
    if cached.get("deep_checked") is not True:
        return report
    if report.get("claude_pid") is not None and not isinstance(
        report.get("claude_start_ticks"), int
    ):
        return report
    for name in (
        "deep_checked",
        "deep_checked_at_unix",
        "sandbox_egress_state",
        "sandbox_egress_failure_stage",
        "sandbox_egress_target",
        "sandbox_egress_canary_identity",
        "sandbox_egress_canary_fingerprint",
        "sandbox_egress_http_status",
        "sandbox_egress_http_statuses",
        "sandbox_contract_stable_during_probe",
        "sandbox_forwarder_probe_count",
        "sandbox_forwarder_passed_count",
        "sandbox_forwarder_failed_count",
        "sandbox_unix_socket_state",
        "sandbox_socks_handshake_state",
        "sandbox_socks_handshake_error",
        "sandbox_egress_curl_exit_code",
        "sandbox_probe_daemon_state",
        "sandbox_probe_daemon_wait_channel",
        "sandbox_probe_daemon_io_blocked",
        "sandbox_probe_daemon_mount_io_blocked",
    ):
        report[name] = cached.get(name)
    return report


def write_cache(report: dict, cache_file: Path) -> bool:
    temporary = cache_file.with_name(f".{cache_file.name}.{os.getpid()}.tmp")
    try:
        cache_file.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(report, handle, ensure_ascii=False, separators=(",", ":"))
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, cache_file)
        return True
    except OSError:
        try:
            temporary.unlink()
        except OSError:
            pass
        return False


def deep_result_is_cacheable(report: Mapping[str, object]) -> bool:
    """Keep transient daemon scheduler observations out of the durable cache."""

    return not deep_result_has_transient_daemon_block(report)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--pid", type=int, help="Claude Science daemon PID to inspect")
    target.add_argument("--current", action="store_true", help="Inspect this process environment")
    parser.add_argument("--deep", action="store_true", help="Run the sandbox egress canary")
    parser.add_argument(
        "--expected-forwarders",
        type=int,
        default=DEFAULT_EXPECTED_SANDBOX_FORWARDERS,
        help="Built-in pair contract (fixed at 3 for Claude Science 0.1.25)",
    )
    parser.add_argument("--cache-file", type=Path)
    parser.add_argument(
        "--cache-max-age",
        type=int,
        default=DEFAULT_CACHE_MAX_AGE_SECONDS,
    )
    parser.add_argument("--write-cache", action="store_true")
    parser.add_argument("--state-only", action="store_true")
    parser.add_argument(
        "--contract-only",
        action="store_true",
        help="Print ready or the first local daemon/sandbox contract failure",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.pid is not None and args.pid <= 0:
        raise SystemExit("--pid must be positive")
    if args.cache_max_age < 0:
        raise SystemExit("--cache-max-age must not be negative")
    if args.expected_forwarders != DEFAULT_EXPECTED_SANDBOX_FORWARDERS:
        raise SystemExit("--expected-forwarders must be exactly 3 for Claude Science 0.1.25")
    if args.write_cache and (not args.deep or args.cache_file is None):
        raise SystemExit("--write-cache requires --deep and --cache-file")
    report = build_report(
        args.pid, args.deep, DEFAULT_CANARY_URL, args.expected_forwarders
    )
    cache_written = True
    if args.deep and args.write_cache and args.cache_file is not None:
        if deep_result_is_cacheable(report):
            cache_written = write_cache(report, args.cache_file)
        else:
            # A previous green result is no longer authoritative after this
            # probe observed the daemon blocked, while publishing the transient
            # D-state itself would make a recovered process look broken for the
            # full cache TTL.  Leave the durable quality state unknown.
            cache_written = invalidate_cache(args.cache_file)
    elif not args.deep and args.cache_file is not None:
        report = merge_fresh_cache(report, args.cache_file, args.cache_max_age)
    if args.contract_only:
        proxy_state = report["proxy_state"]
        if proxy_state not in HEALTHY_PROXY_STATES:
            print(proxy_state)
        elif args.pid is not None and report["sandbox_forwarder_topology_state"] not in {
            "expected",
            "extended",
        }:
            print(f"sandbox_{report['sandbox_forwarder_topology_state']}")
        elif args.deep and report["sandbox_egress_state"] != "ok":
            print(f"egress_{report['sandbox_egress_state']}")
        elif args.write_cache and not cache_written:
            print("cache_write_failed")
        else:
            print("ready")
    elif args.state_only:
        print(report["proxy_state"])
    else:
        json.dump(report, sys.stdout, ensure_ascii=False, separators=(",", ":"))
        print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
