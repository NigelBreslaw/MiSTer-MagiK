from __future__ import annotations

import socket
import threading

import pytest
from magik.client import AgentError, NativeAgent
from magik.protocol import Envelope, receive_message, send_message


def one_reply(
    fields: dict[str, object], operation: str = "status"
) -> tuple[int, threading.Thread]:
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]

    def serve() -> None:
        connection, _ = listener.accept()
        with connection:
            request, _ = receive_message(connection)
            send_message(
                connection, Envelope(request.request_id, operation, "", fields)
            )
        listener.close()

    thread = threading.Thread(target=serve)
    thread.start()
    return port, thread


def test_status_accepts_a_superset_and_unknown_optional_fields() -> None:
    port, thread = one_reply(
        {
            "identity": "other-branch",
            "capabilities": ["status", "upload-v1", "future"],
            "new-field": 1,
        }
    )
    status = NativeAgent("127.0.0.1", "token", port).status()
    thread.join()
    assert status.supports({"status", "upload-v1"})


def test_authentication_error_is_not_retreated_as_bootstrap() -> None:
    port, thread = one_reply({"code": "authentication-failed"}, "error")
    with pytest.raises(AgentError, match="authentication-failed"):
        NativeAgent("127.0.0.1", "wrong", port).status()
    thread.join()


def test_upload_timeout_is_not_automatically_retried(monkeypatch):
    attempts = []

    def unavailable(*args, **kwargs):
        attempts.append(1)
        raise TimeoutError("upload outcome unknown")

    monkeypatch.setattr("magik.client.socket.create_connection", unavailable)
    with pytest.raises(TimeoutError, match="outcome unknown"):
        NativeAgent("localhost", "token").upload(
            "magik", b"app", source_revision="a" * 40, source_dirty=True
        )
    assert len(attempts) == 1


def test_upload_carries_dirty_build_provenance(monkeypatch):
    sent = []

    def request(operation, fields, body, **kwargs):
        sent.append(fields)
        return Envelope("id", "uploaded", "", {}), b""

    agent = NativeAgent("localhost", "token")
    monkeypatch.setattr(agent, "_request", request)
    agent.upload("magik", b"app", source_revision="b" * 40, source_dirty=True)
    assert sent[0]["source_revision"] == "b" * 40
    assert sent[0]["source_dirty"] is True


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


def test_lost_reply_reuses_the_same_request_identifier_once() -> None:
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(2)
    port = listener.getsockname()[1]
    request_ids: list[str] = []

    def serve() -> None:
        first, _ = listener.accept()
        with first:
            request, _ = receive_message(first)
            request_ids.append(request.request_id)
            # The mutation has completed but its acknowledgement is lost.
        second, _ = listener.accept()
        with second:
            request, _ = receive_message(second)
            request_ids.append(request.request_id)
            send_message(
                second, Envelope(request.request_id, "started", "", {"ready": True})
            )
        listener.close()

    thread = threading.Thread(target=serve)
    thread.start()
    assert NativeAgent("127.0.0.1", "token", port).start() == {"ready": True}
    thread.join()
    assert len(request_ids) == 2
    assert request_ids[0] == request_ids[1]


def test_capture_error_is_not_retried():
    port, thread = one_reply({"code": "capture-unavailable"}, "error")
    with pytest.raises(AgentError, match="capture-unavailable"):
        NativeAgent("127.0.0.1", "token", port).capture_framebuffer()
    thread.join()


def test_capture_transport_failure_has_one_attempt(monkeypatch):
    attempts = []

    def fail(*args, **kwargs):
        attempts.append(kwargs["timeout"])
        raise ConnectionRefusedError("offline")

    monkeypatch.setattr(socket, "create_connection", fail)
    with pytest.raises(ConnectionRefusedError):
        NativeAgent("offline", "token").capture_framebuffer()
    assert attempts == [10]


def test_capture_deadline_includes_connect_time(monkeypatch):
    from magik import client

    clock = iter([100, 111])
    monkeypatch.setattr(client.time, "monotonic", lambda: next(clock))

    class Connection:
        def __enter__(self):
            return self

        def __exit__(self, *args):
            pass

    monkeypatch.setattr(
        socket, "create_connection", lambda *args, **kwargs: Connection()
    )
    with pytest.raises(TimeoutError, match="deadline"):
        NativeAgent("fixture", "token").capture_framebuffer()


def test_metrics_preserves_large_evidence_in_body():
    import json

    value = {"window": {"evidence": "x" * (70 * 1024)}}
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]

    def serve():
        connection, _ = listener.accept()
        with connection:
            request, _ = receive_message(connection)
            assert request.operation == "metrics-body"
            send_message(
                connection,
                Envelope(request.request_id, "metrics", "", {"encoding": "json"}),
                json.dumps(value).encode(),
            )
        listener.close()

    thread = threading.Thread(target=serve)
    thread.start()
    assert NativeAgent("127.0.0.1", "token", port).metrics() == value
    thread.join()


def test_metrics_falls_back_only_for_unsupported_body_operation():
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(2)
    port = listener.getsockname()[1]

    def serve():
        for expected, operation, fields in [
            ("metrics-body", "error", {"code": "unsupported-operation"}),
            ("metrics", "metrics", {"presentations": 42}),
        ]:
            connection, _ = listener.accept()
            with connection:
                request, _ = receive_message(connection)
                assert request.operation == expected
                send_message(
                    connection, Envelope(request.request_id, operation, "", fields)
                )
        listener.close()

    thread = threading.Thread(target=serve)
    thread.start()
    assert NativeAgent("127.0.0.1", "token", port).metrics() == {"presentations": 42}
    thread.join()


@pytest.mark.parametrize("in_body", [False, True])
def test_watch_metrics_decode_legacy_headers_and_large_bodies_without_losing_next_event(
    in_body,
):
    import json

    metrics = (
        {"window": {"renderer_profile": "x" * (70 * 1024)}}
        if in_body
        else {"presentations": 42}
    )
    local, peer = socket.socketpair()

    def serve():
        with peer:
            send_message(
                peer,
                Envelope(
                    "watch",
                    "watch-metrics",
                    "",
                    {"encoding": "json"} if in_body else {"metrics": metrics},
                ),
                json.dumps(metrics).encode() if in_body else b"",
            )
            send_message(
                peer, Envelope("watch", "watch-log", "", {"line": "still streaming"})
            )

    thread = threading.Thread(target=serve)
    thread.start()
    try:
        local.settimeout(2)
        event, body = NativeAgent.read_watch_event(local)
        assert event.fields["metrics"] == metrics
        assert body == b""
        assert NativeAgent.read_watch_event(local)[0].operation == "watch-log"
    finally:
        local.close()
        thread.join(timeout=2)
        assert not thread.is_alive()


@pytest.mark.parametrize(
    "body,encoding", [(b"[]", "json"), (b"broken", "json"), (b"{}", "unknown")]
)
def test_watch_rejects_malformed_metrics_bodies(body, encoding):
    from magik.protocol import ProtocolError

    local, peer = socket.socketpair()
    try:
        send_message(
            peer, Envelope("watch", "watch-metrics", "", {"encoding": encoding}), body
        )
        with pytest.raises(ProtocolError):
            NativeAgent.read_watch_event(local)
    finally:
        local.close()
        peer.close()
