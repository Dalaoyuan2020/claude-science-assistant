import importlib.util
import os
import socket
import sys
import threading
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
        assert accepted.wait(timeout=1)
    finally:
        listener.close()
        thread.join(timeout=1)

    assert report["proxy_state"] == "reachable"
    assert report["proxy_reachable"] is True


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
    monkeypatch.setattr(network_quality, "sandbox_forwarders", lambda _pid: [])
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
        "schema_version": 2,
        "claude_pid": 42,
        "claude_start_ticks": 123456,
        "proxy_state": "reachable",
        "proxy_endpoints": ["http://127.0.0.1:10808"],
        "sandbox_forwarder_count": 3,
        "sandbox_forwarder_group_count": 3,
        "sandbox_http_forwarder_count": 3,
        "sandbox_socks_forwarder_count": 3,
        "sandbox_forwarder_expected_count": 3,
        "sandbox_forwarder_topology_state": "expected",
        "sandbox_forwarder_incomplete_extra_count": 0,
        "sandbox_forwarder_fingerprint": "0123456789abcdef",
        "sandbox_probe_identity": network_quality.SANDBOX_PROBE_IDENTITY,
        "sandbox_probe_role": "analysis",
        "sandbox_probe_transport": "socks5h",
        "deep_checked": True,
        "deep_checked_at_unix": 950,
        "sandbox_egress_state": "ok",
        "sandbox_egress_target": "api.github.com",
        "sandbox_egress_canary_identity": "https://api.github.com/zen",
        "sandbox_egress_canary_fingerprint": "a" * 64,
        "sandbox_egress_http_status": 200,
        "sandbox_egress_http_statuses": [200],
        "sandbox_contract_stable_during_probe": True,
        "sandbox_forwarder_probe_count": 1,
        "sandbox_forwarder_passed_count": 1,
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

    changed_transport = {
        **current,
        "sandbox_probe_transport": "http",
        "deep_checked": False,
    }
    rejected = network_quality.merge_fresh_cache(changed_transport, cache_file, 900)
    assert rejected["deep_checked"] is False

    missing_start_identity = {
        **current,
        "claude_start_ticks": None,
        "deep_checked": False,
        "deep_checked_at_unix": None,
    }
    cache_without_start = {**cached, "claude_start_ticks": None}
    assert network_quality.write_cache(cache_without_start, cache_file)
    rejected = network_quality.merge_fresh_cache(missing_start_identity, cache_file, 900)
    assert rejected["deep_checked"] is False


def make_forwarders(count=3, missing_socks_index=None, unstable_index=None):
    processes = []
    for index in range(count):
        base_pid = 100 + index * 2
        start_ticks = 1_000 + index
        if unstable_index == index:
            start_ticks += network_quality.SANDBOX_ROLE_START_TICK_WINDOW + 1
        directory = f"/home/test/.claude-science/sbx-bind-src/sock-42-{index}"
        http_socket = f"{directory}/http.sock"
        socks_socket = f"{directory}/socks.sock"
        processes.append(
            network_quality.SandboxForwarderProcess(
                pid=base_pid,
                start_ticks=start_ticks,
                socket_directory=directory,
                transport="http",
                port=31_000 + index,
                socket_path=http_socket,
                socket_device=1,
                socket_inode=10_000 + index * 2,
            )
        )
        if missing_socks_index != index:
            processes.append(
                network_quality.SandboxForwarderProcess(
                    pid=base_pid + 1,
                    start_ticks=start_ticks,
                    socket_directory=directory,
                    transport="socks",
                    port=32_000 + index,
                    socket_path=socks_socket,
                    socket_device=1,
                    socket_inode=10_001 + index * 2,
                )
            )
    return network_quality.group_sandbox_forwarders(processes)


def test_build_report_rejects_a_contract_change_during_deep_probe(monkeypatch):
    initial = make_forwarders()
    changed = make_forwarders(count=4)
    snapshots = iter((initial, changed))
    monkeypatch.setattr(network_quality, "process_environment", lambda _pid: ({}, "ok"))
    monkeypatch.setattr(network_quality, "process_start_ticks", lambda _pid: 1_000)
    monkeypatch.setattr(network_quality, "sandbox_forwarders", lambda _pid: next(snapshots))
    monkeypatch.setattr(
        network_quality,
        "probe_sandbox_egress",
        lambda *_args: {"deep_checked": True, "sandbox_egress_state": "ok"},
    )
    report = network_quality.build_report(
        42, True, network_quality.DEFAULT_CANARY_URL
    )
    assert report["sandbox_contract_stable_during_probe"] is False
    assert report["sandbox_egress_state"] == "contract_changed"


def run_fake_socks_canary(monkeypatch, forwarders=None, status=204, returncode=0):
    calls = []
    adapter_paths = []
    monkeypatch.setattr(network_quality.shutil, "which", lambda _name: "/usr/bin/curl")
    monkeypatch.setattr(network_quality, "socket_identity_matches", lambda *_args: True)

    class FakeUnixSocketTcpAdapter:
        def __init__(self, socket_path):
            adapter_paths.append(socket_path)
            self.port = 32999
            self.error = None

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return None

    monkeypatch.setattr(network_quality, "UnixSocketTcpAdapter", FakeUnixSocketTcpAdapter)

    def fake_run(command, **kwargs):
        calls.append((command, kwargs))
        return network_quality.subprocess.CompletedProcess(
            command, returncode, stdout=str(status), stderr=""
        )

    monkeypatch.setattr(network_quality.subprocess, "run", fake_run)
    report = network_quality.probe_sandbox_egress(
        make_forwarders() if forwarders is None else forwarders,
        network_quality.DEFAULT_CANARY_URL,
        True,
    )
    return report, calls, adapter_paths


def test_forwarder_command_parser_accepts_only_owned_loopback_socat():
    arguments = [
        "/usr/bin/socat",
        "UNIX-LISTEN:/home/test/.claude-science/sbx-bind-src/sock-42-abc/http.sock,fork,reuseaddr,mode=0600",
        "TCP:localhost:31013,keepalive",
    ]
    sandbox_root = network_quality.PurePosixPath(
        "/home/test/.claude-science/sbx-bind-src"
    )
    parsed = network_quality.parse_sandbox_forwarder_command(
        100, 500, arguments, 42, sandbox_root
    )
    assert parsed is not None
    assert parsed.transport == "http"
    assert parsed.port == 31013
    assert parsed.start_ticks == 500

    foreign = [
        "/usr/bin/socat",
        "UNIX-LISTEN:/tmp/sock-42-abc/http.sock,fork",
        "TCP:localhost:31013",
    ]
    assert (
        network_quality.parse_sandbox_forwarder_command(
            100, 500, foreign, 42, sandbox_root
        )
        is None
    )

    non_loopback = [arguments[0], arguments[1], "TCP:192.0.2.1:31013"]
    assert (
        network_quality.parse_sandbox_forwarder_command(
            100, 500, non_loopback, 42, sandbox_root
        )
        is None
    )


def test_forwarders_are_grouped_before_roles_are_assigned():
    forwarders = make_forwarders()
    assert [item.role for item in forwarders] == ["analysis", "operon", "byoc"]
    assert all(item.complete for item in forwarders)
    assert forwarders[0].http_port == 31000
    assert forwarders[0].socks_port == 32000


def test_deep_canary_uses_only_analysis_socks5h(monkeypatch):
    report, calls, adapter_paths = run_fake_socks_canary(monkeypatch)
    assert report["deep_checked"] is True
    assert report["sandbox_egress_state"] == "ok"
    assert report["sandbox_egress_http_status"] == 204
    assert report["sandbox_forwarder_probe_count"] == 1
    assert report["sandbox_forwarder_passed_count"] == 1
    assert len(calls) == 1
    command, kwargs = calls[0]
    assert command[command.index("--socks5-hostname") + 1] == "127.0.0.1:32999"
    assert adapter_paths == [
        "/home/test/.claude-science/sbx-bind-src/sock-42-0/socks.sock"
    ]
    assert "32000" not in " ".join(command)
    assert "31000" not in " ".join(command)
    assert "32001" not in " ".join(command)
    assert "32002" not in " ".join(command)
    assert kwargs["env"].get("HTTP_PROXY") is None
    assert kwargs["env"].get("NO_PROXY") is None


def test_deep_canary_does_not_misclassify_remote_502_as_local_proxy_failure(monkeypatch):
    report, _calls, _paths = run_fake_socks_canary(monkeypatch, status=502)
    assert report["sandbox_egress_state"] == "http_502"
    assert report["sandbox_egress_http_status"] == 502


def test_deep_canary_target_is_fixed_and_cannot_be_overridden():
    report = network_quality.probe_sandbox_egress(
        make_forwarders(), "http://fixture.invalid/canary", True
    )
    assert report["sandbox_egress_state"] == "invalid_target"
    assert report["sandbox_forwarder_probe_count"] == 0


def test_deep_canary_rejects_incomplete_v0125_forwarder_topology(monkeypatch):
    report, calls, _paths = run_fake_socks_canary(
        monkeypatch, make_forwarders(count=2)
    )
    assert report["sandbox_forwarder_topology_state"] == "incomplete"
    assert report["sandbox_egress_state"] == "topology_incomplete"
    assert report["sandbox_forwarder_passed_count"] == 1
    assert report["sandbox_forwarder_failed_count"] == 0
    assert len(calls) == 1


def test_deep_canary_accepts_all_three_v0125_forwarders(monkeypatch):
    report, _calls, _paths = run_fake_socks_canary(monkeypatch)
    assert report["sandbox_forwarder_topology_state"] == "expected"
    assert report["sandbox_egress_state"] == "ok"
    assert report["sandbox_forwarder_count"] == 3
    assert report["sandbox_http_forwarder_count"] == 3
    assert report["sandbox_socks_forwarder_count"] == 3
    assert report["sandbox_forwarder_passed_count"] == 1
    assert report["sandbox_forwarder_failed_count"] == 0


def test_deep_canary_accepts_complete_extended_topology(monkeypatch):
    report, _calls, _paths = run_fake_socks_canary(
        monkeypatch, make_forwarders(count=4)
    )
    assert report["sandbox_forwarder_topology_state"] == "extended"
    assert report["sandbox_forwarder_count"] == 4
    assert report["sandbox_egress_state"] == "ok"


def test_deep_canary_tolerates_a_transient_incomplete_extra_pair(monkeypatch):
    report, _calls, _paths = run_fake_socks_canary(
        monkeypatch, make_forwarders(count=4, missing_socks_index=3)
    )
    assert report["sandbox_forwarder_topology_state"] == "extended"
    assert report["sandbox_forwarder_count"] == 3
    assert report["sandbox_forwarder_group_count"] == 4
    assert report["sandbox_forwarder_incomplete_extra_count"] == 1
    assert report["sandbox_egress_state"] == "ok"


def test_deep_canary_rejects_replaced_analysis_unix_socket(monkeypatch):
    monkeypatch.setattr(network_quality.shutil, "which", lambda _name: "/usr/bin/curl")
    monkeypatch.setattr(network_quality, "socket_identity_matches", lambda *_args: False)

    class FakeUnixSocketTcpAdapter:
        port = 32999
        error = None

        def __init__(self, _socket_path):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return None

    monkeypatch.setattr(network_quality, "UnixSocketTcpAdapter", FakeUnixSocketTcpAdapter)
    monkeypatch.setattr(
        network_quality.subprocess,
        "run",
        lambda command, **_kwargs: network_quality.subprocess.CompletedProcess(
            command, 0, stdout="200", stderr=""
        ),
    )
    report = network_quality.probe_sandbox_egress(
        make_forwarders(), network_quality.DEFAULT_CANARY_URL, True
    )
    assert report["sandbox_egress_state"] == "failed"
    assert report["sandbox_forwarder_failed_count"] == 1


def test_deep_canary_fails_closed_on_ambiguous_or_unstable_roles(monkeypatch):
    incomplete, _calls, _paths = run_fake_socks_canary(
        monkeypatch, make_forwarders(missing_socks_index=1)
    )
    assert incomplete["sandbox_forwarder_topology_state"] == "incomplete"
    assert incomplete["sandbox_egress_state"] == "topology_incomplete"

    unstable, _calls, _paths = run_fake_socks_canary(
        monkeypatch, make_forwarders(unstable_index=2)
    )
    assert unstable["sandbox_forwarder_topology_state"] == "unstable"
    assert unstable["sandbox_egress_state"] == "topology_unstable"


def test_canary_identity_is_normalized_and_query_bound_without_disclosure():
    _, valid_a, identity_a, fingerprint_a = network_quality.canary_target(
        "HTTPS://API.GITHUB.COM:443/zen?token=first#ignored"
    )
    _, valid_b, identity_b, fingerprint_b = network_quality.canary_target(
        "https://api.github.com/zen?token=second"
    )
    assert valid_a and valid_b
    assert identity_a == identity_b == "https://api.github.com/zen"
    assert fingerprint_a != fingerprint_b
    assert "first" not in fingerprint_a
    assert "second" not in fingerprint_b
