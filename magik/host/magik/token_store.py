"""Local, mode-restricted cache for the native service token.

SSH discovers this token only during bootstrap or repair. Normal native control
traffic reads the local cache and stays on the native connection.
"""

from __future__ import annotations

import hashlib
import os
import tempfile
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
        descriptor, temporary = tempfile.mkstemp(dir=self.path.parent, prefix=".token-")
        try:
            with os.fdopen(descriptor, "w", encoding="utf-8") as output:
                output.write(token + "\n")
            os.replace(temporary, self.path)
        finally:
            Path(temporary).unlink(missing_ok=True)


def state_root() -> Path:
    """Device credentials are shared by worktrees; an explicit override stays available."""
    return Path(
        os.environ.get(
            "MISTER_MAGIK2_STATE",
            str(
                Path(
                    os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state"))
                )
                / "mister-magik2"
            ),
        )
    ).expanduser()
