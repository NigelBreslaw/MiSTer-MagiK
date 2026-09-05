"""Capability negotiation; build identities are deliberately informational."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass


@dataclass(frozen=True)
class AgentStatus:
    identity: str
    capabilities: frozenset[str]
    fields: Mapping[str, object]

    @classmethod
    def from_response(cls, response: Mapping[str, object]) -> "AgentStatus":
        capabilities = response.get("capabilities", [])
        if not isinstance(capabilities, list) or not all(isinstance(item, str) for item in capabilities):
            raise ValueError("agent status has invalid capabilities")
        identity = response.get("identity", "unknown")
        if not isinstance(identity, str):
            raise ValueError("agent status identity must be a string")
        return cls(identity, frozenset(capabilities), response)

    def supports(self, required: Iterable[str]) -> bool:
        return set(required).issubset(self.capabilities)


def needs_install(status: AgentStatus | None, required: Iterable[str]) -> bool:
    return status is None or not status.supports(required)
