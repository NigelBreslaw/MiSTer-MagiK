from __future__ import annotations

import socket
import threading

import pytest

from magik2.client import AgentError, NativeAgent
from magik2.protocol import Envelope, receive_message, send_message


def one_reply(fields: dict[str, object], operation: str = "status") -> tuple[int, threading.Thread]:
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]

    def serve() -> None:
        connection, _ = listener.accept()
        with connection:
            request, _ = receive_message(connection)
            send_message(connection, Envelope(request.request_id, operation, "", fields))
        listener.close()

    thread = threading.Thread(target=serve)
    thread.start()
    return port, thread


def test_status_accepts_a_superset_and_unknown_optional_fields() -> None:
    port, thread = one_reply({"identity": "other-branch", "capabilities": ["status", "upload-v1", "future"], "new-field": 1})
    status = NativeAgent("127.0.0.1", "token", port).status()
    thread.join()
    assert status.supports({"status", "upload-v1"})


def test_authentication_error_is_not_retreated_as_bootstrap() -> None:
    port, thread = one_reply({"code": "authentication-failed"}, "error")
    with pytest.raises(AgentError, match="authentication-failed"):
        NativeAgent("127.0.0.1", "wrong", port).status()
    thread.join()


def test_start_error_retains_launcher_recovery_outcome() -> None:
    port, thread = one_reply({"code": "start-failed", "recovery": None}, "error")
    with pytest.raises(AgentError, match="start-failed; launcher-recovery=passed"):
        NativeAgent("127.0.0.1", "token", port).start()
    thread.join()


def test_test_tunnel_keeps_the_connection_open_after_native_handshake() -> None:
    port, thread = one_reply({"ready": True}, "test-ready")
    tunnel = NativeAgent("127.0.0.1", "token", port).open_test_tunnel()
    tunnel.close()
    thread.join()


def test_watch_keeps_the_connection_open_after_native_handshake() -> None:
    port, thread = one_reply({"ready": True}, "watch-ready")
    watch = NativeAgent("127.0.0.1", "token", port).open_watch()
    watch.close()
    thread.join()
