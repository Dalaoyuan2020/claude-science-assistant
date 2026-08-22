import importlib.util
import os
import socket
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "csa_network_quality", ROOT / "scripts" / "csa-network-quality.py"
)
network_quality = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = network_quality
SPEC.loader.exec_module(network_quality)


def test_proxy_report_redacts_credentials_and_values(monkeypatch):
    monkeypatch.setattr(network_quality, "tcp_reachable", lambda _endpoint: True)
    report = network_quality.inspect_proxy_environment(
        {
            "HTTP_PROXY": "http://alice:secret@proxy.example.test:8080/private",
            "NO_PROXY": "private.example.test",
        }
    )
    encoded = str(report)
    assert report["proxy_state"] == "reachable"
    assert report["proxy_endpoints"] == ["http://proxy.example.test:8080"]
    assert report["proxy_variable_names"] == ["HTTP_PROXY"]
    assert "alice" not in encoded
    assert "secret" not in encoded
    assert "private.example" not in encoded


def test_dead_loopback_proxy_is_unreachable():
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    listener.close()

    report = network_quality.inspect_proxy_environment(
        {"HTTPS_PROXY": f"http://127.0.0.1:{port}"}
    )
    assert report["proxy_state"] == "unreachable"
    assert report["proxy_reachable"] is False


def test_live_loopback_proxy_is_reachable():
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]
    accepted = threading.Event()

    def accept_once():
        connection, _ = listener.accept()
        connection.close()
        accepted.set()

    thread = threading.Thread(target=accept_once, daemon=True)
    thread.start()
    try:
        report = network_quality.inspect_proxy_environment(
            {"http_proxy": f"socks5://127.0.0.1:{port}"}
        )
    finally:
        listener.close()
        thread.join(timeout=1)

    assert report["proxy_state"] == "reachable"
    assert report["proxy_reachable"] is True
    assert accepted.is_set()


def test_direct_environment_requires_no_proxy_listener():
    report = network_quality.inspect_proxy_environment({})
    assert report["proxy_state"] == "direct"
    assert report["proxy_reachable"] is None


def test_conflicting_live_and_dead_loopback_proxies_are_not_green():
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    live_port = listener.getsockname()[1]
    dead_socket = socket.socket()
    dead_socket.bind(("127.0.0.1", 0))
    dead_port = dead_socket.getsockname()[1]
    dead_socket.close()
    thread = threading.Thread(target=lambda: listener.accept()[0].close(), daemon=True)
    thread.start()
    try:
        report = network_quality.inspect_proxy_environment(
            {
                "HTTP_PROXY": f"http://127.0.0.1:{live_port}",
                "HTTPS_PROXY": f"http://127.0.0.1:{dead_port}",
            }
        )
    finally:
        listener.close()
        thread.join(timeout=1)

    assert report["proxy_state"] == "conflict"
    assert report["proxy_reachable"] is False
    assert report["proxy_conflict"] is True


def test_distinct_reachable_proxy_endpoints_are_still_a_conflict(monkeypatch):
    monkeypatch.setattr(network_quality, "tcp_reachable", lambda _endpoint: True)
    report = network_quality.inspect_proxy_environment(
        {
            "HTTP_PROXY": "http://127.0.0.1:10808",
            "http_proxy": "http://127.0.0.1:10809",
        }
    )
    assert report["proxy_state"] == "conflict"
    assert report["proxy_reachable"] is False
    assert report["proxy_conflict"] is True


def test_same_proxy_endpoint_under_multiple_names_is_not_a_conflict(monkeypatch):
    monkeypatch.setattr(network_quality, "tcp_reachable", lambda _endpoint: True)
    report = network_quality.inspect_proxy_environment(
        {
            "HTTP_PROXY": "http://127.0.0.1:10808",
            "http_proxy": "http://127.0.0.1:10808",
        }
    )
    assert report["proxy_state"] == "reachable"
    assert report["proxy_conflict"] is False


def test_process_contract_requires_a_sandbox_forwarder(monkeypatch):
    monkeypatch.setattr(network_quality, "sandbox_http_forwarders", lambda _pid: [])
    monkeypatch.setattr(network_quality, "process_environment", lambda _pid: ({}, "ok"))
    report = network_quality.build_report(1234, False, network_quality.DEFAULT_CANARY_URL)
    assert report["proxy_state"] == "direct"
    assert report["sandbox_forwarder_count"] == 0
    assert report["sandbox_forwarder_expected_count"] == 3
    assert report["sandbox_forwarder_topology_state"] == "incomplete"


def test_process_start_ticks_are_available_for_the_current_process():
    if os.name == "nt":
        pytest.skip("Linux /proc identity is exercised by the WSL test gate")
    ticks = network_quality.process_start_ticks(os.getpid())
    assert isinstance(ticks, int)
    assert ticks > 0


def test_deep_cache_is_reused_only_for_the_same_daemon_contract(tmp_path, monkeypatch):
    monkeypatch.setattr(network_quality.time, "time", lambda: 1_000)
    cached = {
        "schema_version": 1,
        "claude_pid": 42,
        "claude_start_ticks": 123456,
        "proxy_state": "reachable",
        "proxy_endpoints": ["http://127.0.0.1:10808"],
        "sandbox_forwarder_count": 3,
        "sandbox_forwarder_expected_count": 3,
        "sandbox_forwarder_fingerprint": "0123456789abcdef",
        "deep_checked": True,
        "deep_checked_at_unix": 950,
        "sandbox_egress_state": "ok",
        "sandbox_egress_target": "export.arxiv.org",
        "sandbox_egress_canary_identity": "https://export.arxiv.org/api/query",
        "sandbox_egress_canary_fingerprint": "a" * 64,
        "sandbox_egress_http_status": 200,
        "sandbox_egress_http_statuses": [200],
        "sandbox_forwarder_passed_count": 3,
        "sandbox_forwarder_failed_count": 0,
        "secrets_included": False,
    }
    cache_file = tmp_path / "network-quality.json"
    assert network_quality.write_cache(cached, cache_file)

    current = {
        **cached,
        "deep_checked": False,
        "deep_checked_at_unix": None,
        "sandbox_egress_state": "not_checked",
    }
    merged = network_quality.merge_fresh_cache(current, cache_file, 900)
    assert merged["deep_checked"] is True
    assert merged["sandbox_egress_state"] == "ok"

    changed_pid = {**current, "claude_pid": 43, "deep_checked": False}
    rejected = network_quality.merge_fresh_cache(changed_pid, cache_file, 900)
    assert rejected["deep_checked"] is False

    changed_start = {**current, "claude_start_ticks": 123457, "deep_checked": False}
    rejected = network_quality.merge_fresh_cache(changed_start, cache_file, 900)
    assert rejected["deep_checked"] is False

    changed_canary = {
        **current,
        "sandbox_egress_canary_fingerprint": "b" * 64,
        "deep_checked": False,
    }
    rejected = network_quality.merge_fresh_cache(changed_canary, cache_file, 900)
    assert rejected["deep_checked"] is False


def run_fake_proxy_canaries(statuses, expected_forwarders=None):
    servers = []
    threads = []
    for status in statuses:
        class Handler(BaseHTTPRequestHandler):
            response_status = status

            def do_GET(self):
                self.send_response(self.response_status)
                self.send_header("Content-Length", "0")
                self.end_headers()

            def log_message(self, *_args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        servers.append(server)
        threads.append(thread)
    try:
        expected = len(servers) if expected_forwarders is None else expected_forwarders
        return network_quality.probe_sandbox_egress(
            [server.server_address[1] for server in servers],
            "http://fixture.invalid/canary",
            True,
            expected,
        )
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()
        for thread in threads:
            thread.join(timeout=1)


def test_deep_canary_uses_the_supplied_sandbox_http_forwarder():
    report = run_fake_proxy_canaries([204])
    assert report["deep_checked"] is True
    assert report["sandbox_egress_state"] == "ok"
    assert report["sandbox_egress_http_status"] == 204


def test_deep_canary_does_not_misclassify_remote_502_as_local_proxy_failure():
    report = run_fake_proxy_canaries([502])
    assert report["sandbox_egress_state"] == "http_502"
    assert report["sandbox_egress_http_status"] == 502


def test_deep_canary_requires_every_discovered_forwarder():
    report = run_fake_proxy_canaries([204, 204, 502])
    assert report["sandbox_egress_state"] == "partial_failure"
    assert report["sandbox_forwarder_passed_count"] == 2
    assert report["sandbox_forwarder_failed_count"] == 1
    assert report["sandbox_egress_http_statuses"] == [204, 502]


def test_deep_canary_rejects_incomplete_v0125_forwarder_topology():
    report = run_fake_proxy_canaries([204, 204], expected_forwarders=3)
    assert report["sandbox_forwarder_topology_state"] == "incomplete"
    assert report["sandbox_egress_state"] == "topology_incomplete"
    assert report["sandbox_forwarder_passed_count"] == 2
    assert report["sandbox_forwarder_failed_count"] == 0


def test_deep_canary_accepts_all_three_v0125_forwarders():
    report = run_fake_proxy_canaries([204, 204, 204], expected_forwarders=3)
    assert report["sandbox_forwarder_topology_state"] == "expected"
    assert report["sandbox_egress_state"] == "ok"
    assert report["sandbox_forwarder_passed_count"] == 3
    assert report["sandbox_forwarder_failed_count"] == 0


def test_canary_identity_is_normalized_and_query_bound_without_disclosure():
    _, valid_a, identity_a, fingerprint_a = network_quality.canary_target(
        "HTTPS://EXPORT.ARXIV.ORG:443/api/query?token=first#ignored"
    )
    _, valid_b, identity_b, fingerprint_b = network_quality.canary_target(
        "https://export.arxiv.org/api/query?token=second"
    )
    assert valid_a and valid_b
    assert identity_a == identity_b == "https://export.arxiv.org/api/query"
    assert fingerprint_a != fingerprint_b
    assert "first" not in fingerprint_a
    assert "second" not in fingerprint_b
