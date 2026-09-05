"""Native-only host client; streaming will use independent connections."""

from __future__ import annotations

import socket
import uuid
from collections.abc import Mapping

from .compatibility import AgentStatus
from .protocol import Envelope, ProtocolError, receive_message, send_message, sha256_hex


class AgentError(RuntimeError):
    pass


class NativeAgent:
    def __init__(self, host: str, token: str, port: int = 7500) -> None:
        self.host = host
        self.token = token
        self.port = port

    def status(self) -> AgentStatus:
        response, _ = self._request("status")
        if response.operation == "error":
            raise AgentError(str(response.fields.get("code", "agent error")))
        return AgentStatus.from_response(response.fields)

    def upload(self, artifact: str, payload: bytes) -> Mapping[str, object]:
        response, _ = self._request("upload", {"artifact": artifact, "sha256": sha256_hex(payload)}, payload)
        if response.operation == "error":
            raise AgentError(str(response.fields.get("code", "upload failed")))
        return response.fields

    def start(self, *, restart: bool = False) -> Mapping[str, object]:
        return self._successful("start", {"restart": restart})

    def stop(self) -> Mapping[str, object]:
        return self._successful("stop")

    def metrics(self) -> Mapping[str, object]:
        return self._successful("metrics")

    def open_test_tunnel(self) -> socket.socket:
        request = Envelope(uuid.uuid4().hex, "test-start", self.token, {})
        connection = socket.create_connection((self.host, self.port), timeout=20)
        try:
            send_message(connection, request)
            response, body = receive_message(connection)
            if response.request_id != request.request_id:
                raise ProtocolError("agent reply request identifier did not match")
            if body or response.operation == "error":
                raise AgentError(str(response.fields.get("code", "test bridge failed")))
            if response.operation != "test-ready":
                raise AgentError("agent did not establish a test bridge")
            return connection
        except Exception:
            connection.close()
            raise

    def _successful(self, operation: str, fields: Mapping[str, object] | None = None) -> Mapping[str, object]:
        response, _ = self._request(operation, fields)
        if response.operation == "error":
            raise AgentError(str(response.fields.get("code", f"{operation} failed")))
        return response.fields

    def _request(self, operation: str, fields: Mapping[str, object] | None = None, body: bytes = b"") -> tuple[Envelope, bytes]:
        request = Envelope(uuid.uuid4().hex, operation, self.token, fields or {})
        with socket.create_connection((self.host, self.port), timeout=5) as connection:
            send_message(connection, request, body)
            response, response_body = receive_message(connection)
        if response.request_id != request.request_id:
            raise ProtocolError("agent reply request identifier did not match")
        return response, response_body
