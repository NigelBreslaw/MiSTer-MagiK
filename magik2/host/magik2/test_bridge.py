"""Local subprocess used by slint-testing to reach the native test tunnel."""

from __future__ import annotations

import os
import socket
import threading
import argparse

from .client import AgentError, NativeAgent


def _test_server() -> tuple[str, int]:
    value = os.environ.get("SLINT_TEST_SERVER", "")
    host, separator, port = value.rpartition(":")
    if not separator or not host or not port.isdecimal():
        raise RuntimeError("SLINT_TEST_SERVER must be host:port")
    return host, int(port)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile-id")
    arguments = parser.parse_args()
    host, port = _test_server()
    token = os.environ.get("MISTER_MAGIK2_TOKEN")
    device = os.environ.get("MISTER_IP")
    if not token or not device:
        raise RuntimeError(
            "MISTER_IP and MISTER_MAGIK2_TOKEN are required for the native test bridge"
        )
    native_port = int(os.environ.get("MISTER_MAGIK2_PORT", "7500"))
    agent = NativeAgent(device, token, native_port)
    agent.artifact = os.environ.get("MISTER_MAGIK2_APP", "mini-magik")
    agent.expected_sha256 = os.environ.get("MISTER_MAGIK2_EXPECTED_SHA256")
    with agent.open_test_tunnel(arguments.profile_id) as device_connection:
        with socket.create_connection((host, port), timeout=10) as testing_connection:
            device_connection.settimeout(None)
            testing_connection.settimeout(None)
            upstream = threading.Thread(
                target=_copy,
                args=(device_connection, testing_connection),
                daemon=True,
            )
            upstream.start()
            _copy(testing_connection, device_connection)
            upstream.join(timeout=2)
    return 0


def _copy(source: socket.socket, destination: socket.socket) -> None:
    try:
        while chunk := source.recv(64 * 1024):
            destination.sendall(chunk)
    finally:
        try:
            destination.shutdown(socket.SHUT_WR)
        except OSError:
            pass


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AgentError, OSError, RuntimeError) as error:
        print(f"magik2 test bridge: {error}", file=os.sys.stderr)
        raise SystemExit(2) from error
