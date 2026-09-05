from __future__ import annotations

import socket

import pytest
from magik2.compatibility import AgentStatus, needs_install
from magik2.protocol import (
    Envelope,
    ProtocolError,
    receive_message,
    send_message,
    sha256_hex,
)


def test_binary_body_round_trips_without_json_encoding() -> None:
    left, right = socket.socketpair()
    try:
        send_message(
            left,
            Envelope(
                "request-1", "upload", "secret", {"sha256": sha256_hex(b"\x00\xff")}
            ),
            b"\x00\xff",
        )
        envelope, body = receive_message(right)
    finally:
        left.close()
        right.close()
    assert envelope.operation == "upload"
    assert body == b"\x00\xff"


def test_truncated_body_is_rejected() -> None:
    left, right = socket.socketpair()
    try:
        header = Envelope("request-1", "upload", "secret", {}).to_json()
        left.sendall(
            len(header).to_bytes(4, "big") + (4).to_bytes(8, "big") + header + b"x"
        )
        left.close()
        with pytest.raises(ProtocolError, match="closed"):
            receive_message(right)
    finally:
        right.close()


def test_capabilities_not_build_identity_select_agent() -> None:
    older = AgentStatus.from_response(
        {
            "identity": "branch-a",
            "capabilities": ["status", "upload", "start"],
            "future": True,
        }
    )
    assert not needs_install(older, {"status", "upload"})
    assert needs_install(older, {"frames"})


def test_total_deadline_expires_even_when_peer_keeps_sending(monkeypatch):
    from magik2 import protocol

    clock = iter([0, 9, 11])
    monkeypatch.setattr(protocol.time, "monotonic", lambda: next(clock))

    class SlowPeer:
        def settimeout(self, seconds):
            assert seconds > 0

        def recv(self, size):
            return b"x"

    with pytest.raises(TimeoutError, match="deadline"):
        protocol._read_exact(SlowPeer(), 4, deadline=10)
