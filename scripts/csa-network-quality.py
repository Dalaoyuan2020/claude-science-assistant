#!/usr/bin/env python3
"""Redacted proxy and Claude Science sandbox egress diagnostics.

The default probe is local-only: it reads a process environment, reports only
proxy variable names and credential-free endpoints, and checks loopback proxy
TCP listeners.  ``--deep`` additionally sends a no-auth, no-billing canary
request through Claude Science's own sandbox HTTP forwarders.
"""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
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
HEALTHY_PROXY_STATES = {"direct", "reachable"}
DEFAULT_CANARY_URL = (
    "https://export.arxiv.org/api/query?search_query=all:electron&start=0&max_results=1"
)
DEFAULT_CACHE_MAX_AGE_SECONDS = 15 * 60
DEFAULT_EXPECTED_SANDBOX_FORWARDERS = 3


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


def sandbox_http_forwarders(claude_pid: int) -> list[int]:
    ports = set()
    for process_dir in Path("/proc").iterdir():
        if not process_dir.name.isdigit():
            continue
        pid = int(process_dir.name)
        if process_parent_pid(pid) != claude_pid:
            continue
        try:
            command = (process_dir / "cmdline").read_bytes().replace(b"\0", b" ").decode(
                "utf-8", errors="replace"
            )
        except OSError:
            continue
        if "socat" not in command or "/http.sock" not in command:
            continue
        match = re.search(r"\bTCP:(?:localhost|127\.0\.0\.1):(\d+)\b", command)
        if match:
            port = int(match.group(1))
            if 1 <= port <= 65535:
                ports.add(port)
    return sorted(ports)


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
    ports: list[int],
    url: str,
    deep: bool,
    expected_forwarders: int = DEFAULT_EXPECTED_SANDBOX_FORWARDERS,
) -> dict:
    target, valid_target, canary_identity, canary_fingerprint = canary_target(url)
    topology_state = (
        "incomplete"
        if len(ports) < expected_forwarders
        else "expected"
        if len(ports) == expected_forwarders
        else "unexpected_extra"
    )
    result = {
        "deep_checked": bool(deep),
        "sandbox_forwarder_count": len(ports),
        "sandbox_forwarder_expected_count": expected_forwarders,
        "sandbox_forwarder_topology_state": topology_state,
        "sandbox_forwarder_fingerprint": (
            hashlib.sha256(",".join(str(port) for port in ports).encode()).hexdigest()[:16]
            if ports
            else None
        ),
        "sandbox_egress_state": "not_checked",
        "sandbox_egress_target": target,
        "sandbox_egress_canary_identity": canary_identity,
        "sandbox_egress_canary_fingerprint": canary_fingerprint,
        "sandbox_egress_http_status": None,
        "sandbox_egress_http_statuses": [],
        "sandbox_forwarder_passed_count": 0,
        "sandbox_forwarder_failed_count": 0,
    }
    if not deep:
        return result
    if not valid_target:
        result["sandbox_egress_state"] = "invalid_target"
        return result
    if not ports:
        result["sandbox_egress_state"] = "unavailable"
        return result
    curl = shutil.which("curl")
    if not curl:
        result["sandbox_egress_state"] = "unavailable"
        return result

    attempts = []
    for port in ports:
        command = [
            curl,
            "--silent",
            "--show-error",
            "--output",
            os.devnull,
            "--write-out",
            "%{http_code}",
            "--noproxy",
            "",
            "--proxy",
            f"http://127.0.0.1:{port}",
            "--connect-timeout",
            "1",
            "--max-time",
            "4",
            "--retry",
            "0",
            "--user-agent",
            "CSA-Network-Quality/1",
            url,
        ]
        clean_environment = dict(os.environ)
        for name in PROXY_VARIABLES:
            clean_environment.pop(name, None)
        try:
            completed = subprocess.run(
                command,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=5,
                check=False,
                env=clean_environment,
            )
        except (OSError, subprocess.TimeoutExpired):
            attempts.append((0, False))
            continue
        try:
            status = int(completed.stdout.strip() or "0")
        except ValueError:
            status = 0
        attempts.append((status, completed.returncode == 0 and 200 <= status < 300))

    statuses = [status for status, _passed in attempts]
    passed_count = sum(1 for _status, passed in attempts if passed)
    failed_count = len(attempts) - passed_count
    nonzero_statuses = [status for status in statuses if status]
    unique_statuses = sorted(set(nonzero_statuses))
    result["sandbox_egress_http_statuses"] = unique_statuses
    result["sandbox_forwarder_passed_count"] = passed_count
    result["sandbox_forwarder_failed_count"] = failed_count

    if topology_state == "incomplete":
        result["sandbox_egress_state"] = "topology_incomplete"
    elif passed_count == len(ports):
        result["sandbox_egress_state"] = "ok"
    elif passed_count:
        result["sandbox_egress_state"] = "partial_failure"
    elif unique_statuses == [502]:
        result["sandbox_egress_state"] = "http_502"
    elif unique_statuses == [403]:
        result["sandbox_egress_state"] = "http_403"
    elif len(unique_statuses) > 1:
        result["sandbox_egress_state"] = "mixed_http_failure"
    else:
        result["sandbox_egress_state"] = "failed"
    if len(unique_statuses) == 1:
        result["sandbox_egress_http_status"] = unique_statuses[0]
    return result


def build_report(
    pid: Optional[int],
    deep: bool,
    canary_url: str,
    expected_forwarders: int = DEFAULT_EXPECTED_SANDBOX_FORWARDERS,
) -> dict:
    if pid is None:
        proxy_report = inspect_proxy_environment(os.environ)
        forwarders = []
        start_ticks = None
    else:
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
        forwarders = sandbox_http_forwarders(pid) if process_state == "ok" else []
        start_ticks = process_start_ticks(pid) if process_state == "ok" else None

    return {
        "schema_version": 1,
        "claude_pid": pid,
        "claude_start_ticks": start_ticks,
        **proxy_report,
        **probe_sandbox_egress(forwarders, canary_url, deep, expected_forwarders),
        "deep_checked_at_unix": int(time.time()) if deep else None,
        "secrets_included": False,
    }


def merge_fresh_cache(report: dict, cache_file: Path, max_age_seconds: int) -> dict:
    try:
        cached = json.loads(cache_file.read_text(encoding="utf-8"))
    except (OSError, ValueError, TypeError):
        return report
    checked_at = cached.get("deep_checked_at_unix")
    if not isinstance(checked_at, int):
        return report
    age = int(time.time()) - checked_at
    if age < 0 or age > max_age_seconds:
        return report
    identity_fields = (
        "claude_pid",
        "claude_start_ticks",
        "proxy_state",
        "proxy_endpoints",
        "sandbox_forwarder_count",
        "sandbox_forwarder_expected_count",
        "sandbox_forwarder_fingerprint",
        "sandbox_egress_canary_fingerprint",
    )
    if any(cached.get(name) != report.get(name) for name in identity_fields):
        return report
    if cached.get("deep_checked") is not True:
        return report
    for name in (
        "deep_checked",
        "deep_checked_at_unix",
        "sandbox_egress_state",
        "sandbox_egress_target",
        "sandbox_egress_canary_identity",
        "sandbox_egress_canary_fingerprint",
        "sandbox_egress_http_status",
        "sandbox_egress_http_statuses",
        "sandbox_forwarder_passed_count",
        "sandbox_forwarder_failed_count",
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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--pid", type=int, help="Claude Science daemon PID to inspect")
    target.add_argument("--current", action="store_true", help="Inspect this process environment")
    parser.add_argument("--deep", action="store_true", help="Run the sandbox egress canary")
    parser.add_argument("--canary-url", default=DEFAULT_CANARY_URL)
    parser.add_argument(
        "--expected-forwarders",
        type=int,
        default=DEFAULT_EXPECTED_SANDBOX_FORWARDERS,
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
    if args.expected_forwarders < 1:
        raise SystemExit("--expected-forwarders must be positive")
    if args.write_cache and (not args.deep or args.cache_file is None):
        raise SystemExit("--write-cache requires --deep and --cache-file")
    report = build_report(args.pid, args.deep, args.canary_url, args.expected_forwarders)
    cache_written = True
    if args.deep and args.write_cache and args.cache_file is not None:
        cache_written = write_cache(report, args.cache_file)
    elif not args.deep and args.cache_file is not None:
        report = merge_fresh_cache(report, args.cache_file, args.cache_max_age)
    if args.contract_only:
        proxy_state = report["proxy_state"]
        if proxy_state not in HEALTHY_PROXY_STATES:
            print(proxy_state)
        elif (
            args.pid is not None
            and report["sandbox_forwarder_count"]
            < report["sandbox_forwarder_expected_count"]
        ):
            print("sandbox_incomplete")
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
