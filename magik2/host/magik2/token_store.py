"""Local, mode-restricted cache for the native service token.

SSH discovers this token only during bootstrap or repair. Normal 2.0 control
traffic reads the local cache and stays on the native connection.
"""

from __future__ import annotations

import hashlib
import os
from pathlib import Path


class TokenStore:
    def __init__(self, root: Path, device: str) -> None:
        self.path = root / f"{hashlib.sha256(device.encode()).hexdigest()[:16]}.token"

    def load(self) -> str | None:
        try:
            token = self.path.read_text(encoding="utf-8").strip()
        except FileNotFoundError:
            return None
        return token or None

    def save(self, token: str) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temporary = self.path.with_suffix(".next")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(token + "\n")
        os.replace(temporary, self.path)
