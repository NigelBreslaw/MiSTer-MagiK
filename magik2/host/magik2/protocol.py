"""The bounded native control envelope and binary payload framing."""

from __future__ import annotations

import hashlib
import json
import socket
import struct
import time
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

MAX_HEADER_BYTES = 64 * 1024
MAX_BODY_BYTES = 64 * 1024 * 1024


class ProtocolError(RuntimeError):
    """The peer sent malformed or unsafe framed data."""


def _read_exact(
    connection: socket.socket, size: int, deadline: float | None = None
) -> bytes:
    result = bytearray()
    while len(result) < size:
        if deadline is not None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("capture deadline exceeded")
            connection.settimeout(remaining)
        chunk = connection.recv(size - len(result))
        if not chunk:
            raise ProtocolError("connection closed while reading a framed message")
        result.extend(chunk)
    return bytes(result)


@dataclass(frozen=True)
class Envelope:
    request_id: str
    operation: str
    token: str
    fields: Mapping[str, Any]

    def to_json(self) -> bytes:
        data = {
            "id": self.request_id,
            "op": self.operation,
            "token": self.token,
            **self.fields,
        }
        encoded = json.dumps(data, sort_keys=True, separators=(",", ":")).encode()
        if len(encoded) > MAX_HEADER_BYTES:
            raise ProtocolError("control header exceeds 64 KiB")
        return encoded

    @classmethod
    def from_json(cls, raw: bytes) -> Envelope:
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError as error:
            raise ProtocolError("control header is not JSON") from error
        if not isinstance(parsed, dict) or not all(
            isinstance(parsed.get(key), str) for key in ("id", "op", "token")
        ):
            raise ProtocolError("control header lacks string id, op, or token")
        return cls(parsed.pop("id"), parsed.pop("op"), parsed.pop("token"), parsed)


def send_message(
    connection: socket.socket, envelope: Envelope, body: bytes = b""
) -> None:
    if len(body) > MAX_BODY_BYTES:
        raise ProtocolError("binary body exceeds configured limit")
    header = envelope.to_json()
    connection.sendall(struct.pack("!IQ", len(header), len(body)) + header)
    view = memoryview(body)
    for offset in range(0, len(body), 64 * 1024):
        connection.sendall(view[offset : offset + 64 * 1024])


def receive_message(
    connection: socket.socket, *, deadline: float | None = None
) -> tuple[Envelope, bytes]:
    header_size, body_size = struct.unpack("!IQ", _read_exact(connection, 12, deadline))
    if header_size > MAX_HEADER_BYTES or body_size > MAX_BODY_BYTES:
        raise ProtocolError("peer declared an oversized message")
    return Envelope.from_json(
        _read_exact(connection, header_size, deadline)
    ), _read_exact(connection, body_size, deadline)


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()
