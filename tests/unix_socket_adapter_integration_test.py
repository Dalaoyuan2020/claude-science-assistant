#!/usr/bin/env python3
"""Exercise the real TCP-to-Unix adapter with a minimal SOCKS5 exchange."""

from __future__ import annotations

import importlib.util
import socket
import sys
import tempfile
import threading
from pathlib import Path


def receive_exact(connection: socket.socket, length: int) -> bytes:
    payload = bytearray()
    while len(payload) < length:
        chunk = connection.recv(length - len(payload))
        if not chunk:
            raise RuntimeError("unexpected EOF")
        payload.extend(chunk)
    return bytes(payload)


def load_network_quality(project_root: Path):
    path = project_root / "scripts" / "csa-network-quality.py"
    spec = importlib.util.spec_from_file_location("csa_network_quality_adapter_test", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: unix_socket_adapter_integration_test.py PROJECT_ROOT")
    project_root = Path(sys.argv[1]).resolve()
    network_quality = load_network_quality(project_root)
    ready = threading.Event()
    errors: list[BaseException] = []
    observed: dict[str, object] = {}

    with tempfile.TemporaryDirectory(prefix="csa-unix-adapter-") as temporary:
        unix_path = Path(temporary) / "analysis-socks.sock"

        def serve_socks() -> None:
            server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                server.bind(str(unix_path))
                server.listen(1)
                ready.set()
                connection, _address = server.accept()
                with connection:
                    greeting = receive_exact(connection, 3)
                    if greeting != b"\x05\x01\x00":
                        raise RuntimeError(f"unexpected SOCKS greeting: {greeting!r}")
                    connection.sendall(b"\x05\x00")
                    header = receive_exact(connection, 5)
                    if header[:4] != b"\x05\x01\x00\x03":
                        raise RuntimeError(f"unexpected SOCKS request: {header!r}")
                    host = receive_exact(connection, header[4]).decode("ascii")
                    port = int.from_bytes(receive_exact(connection, 2), "big")
                    observed.update(host=host, port=port)
                    connection.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
                    if receive_exact(connection, 4) != b"PING":
                        raise RuntimeError("relay request payload was corrupted")
                    connection.sendall(b"PONG")
            except BaseException as error:  # surfaced in the main thread below
                errors.append(error)
                ready.set()
            finally:
                server.close()

        server_thread = threading.Thread(target=serve_socks, daemon=True)
        server_thread.start()
        if not ready.wait(timeout=3):
            raise RuntimeError("Unix SOCKS fixture did not start")
        if errors:
            raise errors[0]

        with network_quality.UnixSocketTcpAdapter(str(unix_path)) as adapter:
            with socket.create_connection(("127.0.0.1", adapter.port), timeout=3) as client:
                client.sendall(b"\x05\x01\x00")
                if receive_exact(client, 2) != b"\x05\x00":
                    raise RuntimeError("adapter did not relay the SOCKS greeting")
                host = b"api.github.com"
                client.sendall(
                    b"\x05\x01\x00\x03"
                    + bytes((len(host),))
                    + host
                    + (443).to_bytes(2, "big")
                )
                if receive_exact(client, 10)[:2] != b"\x05\x00":
                    raise RuntimeError("adapter did not relay the SOCKS connect response")
                client.sendall(b"PING")
                if receive_exact(client, 4) != b"PONG":
                    raise RuntimeError("adapter did not relay the response payload")
        server_thread.join(timeout=3)
        if server_thread.is_alive():
            raise RuntimeError("Unix SOCKS fixture did not stop")
        if errors:
            raise errors[0]
        if observed != {"host": "api.github.com", "port": 443}:
            raise RuntimeError(f"SOCKS hostname/port changed in transit: {observed!r}")

    print("Unix socket adapter integration test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
