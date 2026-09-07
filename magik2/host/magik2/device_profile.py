"""Shared non-secret device selection, independent of checkout and DHCP address."""

from __future__ import annotations

import json
import os
import re
import tempfile
from dataclasses import asdict, dataclass
from pathlib import Path

from .token_store import state_root


def device_identity(value: object) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"(?:[0-9a-f]{2}:){5}[0-9a-f]{2}", value.lower()):
        raise ValueError("MiSTer returned an invalid device identity")
    value = value.lower()
    if value in {"00:00:00:00:00:00", "ff:ff:ff:ff:ff:ff"}:
        raise ValueError("MiSTer returned an invalid device identity")
    return value


@dataclass(frozen=True)
class DeviceProfile:
    identity: str
    address: str
    username: str

    @classmethod
    def load(cls, root: Path | None = None) -> DeviceProfile | None:
        try:
            fields = json.loads(((root or state_root()) / "device.json").read_text())
        except FileNotFoundError:
            return None
        if not isinstance(fields, dict) or set(fields) != {"identity", "address", "username"}:
            raise ValueError("invalid remembered MiSTer configuration")
        profile = cls(**fields)
        device_identity(profile.identity)
        if not all(isinstance(value, str) and value for value in (profile.address, profile.username)):
            raise ValueError("invalid remembered MiSTer configuration")
        return profile

    def save(self, root: Path | None = None) -> None:
        root = root or state_root()
        root.mkdir(parents=True, exist_ok=True)
        descriptor, temporary = tempfile.mkstemp(dir=root, prefix=".device-")
        try:
            with os.fdopen(descriptor, "w") as output:
                json.dump(asdict(self), output)
                output.write("\n")
            os.replace(temporary, root / "device.json")
        finally:
            Path(temporary).unlink(missing_ok=True)


def migrate_token(profile: DeviceProfile, root: Path | None = None) -> str | None:
    """Call only after verifying the address belongs to this identity."""
    from .token_store import TokenStore

    root = root or state_root()
    target = TokenStore(root, profile.identity)
    old = TokenStore(root, profile.address)
    token = target.load()
    if token is None:
        token = old.load()
        if token is not None:
            target.save(token)
    if token is not None and old.load() == token:
        old.path.unlink(missing_ok=True)
    return token
