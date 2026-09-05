"""Native-only host client; streaming will use independent connections."""

from __future__ import annotations

import socket
import uuid
from collections.abc import Mapping

from .compatibility import AgentStatus
from .protocol import Envelope, ProtocolError, receive_message, send_message, sha256_hex


class AgentError(RuntimeError):
    @classmethod
    def from_fields(cls, fields):
        code = str(fields.get("code", "agent-error"))
        detail = fields.get("detail")
        recovery = fields.get("recovery", "not-attempted")
        text = f"{code}: {detail}" if detail else code
        if "recovery" in fields:
            text += f"; launcher-recovery={'passed' if recovery is None else 'failed: ' + str(recovery)}"
        return cls(text)


class NativeAgent:
    def __init__(self, host: str, token: str, port: int = 7500) -> None:
        self.host = host
        self.token = token
        self.port = port
        self.expected_sha256: str | None = None
        self.artifact = "mini-magik"

    def status(self) -> AgentStatus:
        response, _ = self._request("status")
        if response.operation == "error":
            raise AgentError.from_fields(response.fields)
        return AgentStatus.from_response(response.fields)

    def upload(self, artifact: str, payload: bytes) -> Mapping[str, object]:
        response, _ = self._request(
            "upload", {"artifact": artifact, "sha256": sha256_hex(payload)}, payload
        )
        if response.operation == "error":
            raise AgentError.from_fields(response.fields)
        return response.fields

    def upgrade_agent(self, payload: bytes) -> Mapping[str, object]:
        response, _ = self._request(
            "agent-update", {"sha256": sha256_hex(payload)}, payload
        )
        if response.operation == "error":
            raise AgentError.from_fields(response.fields)
        if response.operation != "agent-updating":
            raise AgentError("agent did not acknowledge its replacement")
        return response.fields

    def start(
        self, *, restart: bool = False, expected_sha256: str | None = None
    ) -> Mapping[str, object]:
        fields: dict[str, object] = {"restart": restart, "artifact": self.artifact}
        if expected_sha256 is not None:
            fields["expected_sha256"] = expected_sha256
        return self._successful("start", fields)

    def stop(self) -> Mapping[str, object]:
        return self._successful("stop")

    def diagnostics(self) -> Mapping[str, object]:
        return self._successful("diagnostics")

    def metrics(self) -> Mapping[str, object]:
        return self._successful("metrics")

    def read_profile_artifact(self, profile_id: str, name: str) -> bytes:
        response, body = self._request(
            "read-artifact", {"profile_id": profile_id, "name": name}
        )
        if response.operation == "error":
            raise AgentError.from_fields(response.fields)
        if response.operation != "artifact":
            raise AgentError("agent returned an unexpected artifact response")
        return body

    def open_test_tunnel(self, profile_id: str | None = None) -> socket.socket:
        fields = {"artifact": self.artifact}
        if profile_id is not None:
            fields["profile_id"] = profile_id
        if self.expected_sha256 is not None:
            fields["expected_sha256"] = self.expected_sha256
        request = Envelope(uuid.uuid4().hex, "test-start", self.token, fields)
        connection = socket.create_connection((self.host, self.port), timeout=20)
        try:
            send_message(connection, request)
            response, body = receive_message(connection)
            if response.request_id != request.request_id:
                raise ProtocolError("agent reply request identifier did not match")
            if body or response.operation == "error":
                raise AgentError.from_fields(response.fields)
            if response.operation != "test-ready":
                raise AgentError("agent did not establish a test bridge")
            return connection
        except Exception:
            connection.close()
            raise

    def open_watch(self) -> socket.socket:
        request = Envelope(uuid.uuid4().hex, "watch", self.token, {})
        connection = socket.create_connection((self.host, self.port), timeout=20)
        try:
            send_message(connection, request)
            response, body = receive_message(connection)
            if response.request_id != request.request_id:
                raise ProtocolError("agent reply request identifier did not match")
            if body or response.operation == "error":
                raise AgentError.from_fields(response.fields)
            if response.operation != "watch-ready":
                raise AgentError("agent did not establish observation stream")
            connection.settimeout(None)
            return connection
        except Exception:
            connection.close()
            raise

    @staticmethod
    def read_watch_event(connection: socket.socket) -> tuple[Envelope, bytes]:
        response, body = receive_message(connection)
        if response.operation not in {"watch-metrics", "watch-log", "watch-frame"}:
            raise AgentError(f"unexpected watch event: {response.operation}")
        return response, body

    def _successful(
        self, operation: str, fields: Mapping[str, object] | None = None
    ) -> Mapping[str, object]:
        response, _ = self._request(operation, fields)
        if response.operation == "error":
            raise AgentError.from_fields(response.fields)
        return response.fields

    def _request(
        self,
        operation: str,
        fields: Mapping[str, object] | None = None,
        body: bytes = b"",
    ) -> tuple[Envelope, bytes]:
        request = Envelope(uuid.uuid4().hex, operation, self.token, fields or {})
        last_error: OSError | ProtocolError | None = None
        attempts = (
            1 if operation in {"agent-update", "transfer-check", "measure"} else 2
        )
        for attempt in range(attempts):
            try:
                with socket.create_connection(
                    (self.host, self.port), timeout=5
                ) as connection:
                    connection.settimeout(25 if operation == "start" else 15)
                    send_message(connection, request, body)
                    response, response_body = receive_message(connection)
                if response.request_id != request.request_id:
                    raise ProtocolError("agent reply request identifier did not match")
                return response, response_body
            except (OSError, ProtocolError) as error:
                last_error = error
                if attempt == attempts - 1:
                    raise
        assert last_error is not None
        raise last_error
