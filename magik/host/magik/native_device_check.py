"""Explicit device-plane controls and authoritative scanout capture journey."""

from __future__ import annotations

import json
import time
from pathlib import Path

from .animation_benchmark import _focus, _key, _tree
from .results import append_event


def device_plane_journey(app, agent, run: Path):
    def settle():
        time.sleep(1.5)
        deadline = time.monotonic() + 8
        while agent.metrics().get("ui_motion"):
            assert time.monotonic() < deadline, "Navigation did not settle"
            time.sleep(0.1)

    def capture(name):
        metadata, pixels = agent.capture_framebuffer()
        assert metadata["source"] == "fpga-latched-scanout-slots"
        assert metadata["pixel_format"] == "rgb565-le"
        (run / f"device-plane-{name}.rgb565").write_bytes(pixels)
        (run / f"device-plane-{name}.json").write_text(json.dumps(metadata, indent=2))
        append_event(
            run,
            {
                "phase": "device-plane-capture",
                "name": name,
                "metadata": metadata,
                "tree": _tree(app),
            },
        )

    _key(app, "\uf729")
    settle()
    _focus(app, r"^Consoles$")
    _key(app, "\n")
    # Taps while the level trick is locked must not enter another level.
    time.sleep(0.1)
    _key(app, "\n")
    settle()
    assert _tree(app)["menu"] == "Consoles"
    _focus(app, r"^Nintendo$")
    _key(app, "\n")
    settle()
    _focus(app, r"^SNES$")
    _key(app, "\n")
    settle()
    assert "GAMES" in _tree(app)["selected"]
    capture("hub")
    _key(app, "\n")
    settle()
    assert any(e["label"] == "Arcade games" for e in _tree(app)["elements"])
    capture("games")
    # Dev keyboard Tab enters the same Select action queue as the UI intent.
    _key(app, "\t")
    settle()
    assert "GAMES" in _tree(app)["selected"], "Select did not restore the hub"
    capture("select-hub")
    _key(app, "\x1b")
    settle()
    assert "Nintendo" in _tree(app)["menu"]
    _key(app, "\x1b")
    settle()
    assert _tree(app)["menu"] == "Consoles"
    _key(app, "\x1b")
    settle()
    assert "Consoles" in _tree(app)["selected"]
    capture("root")
    append_event(
        run,
        {
            "phase": "device-plane-controls",
            "outcome": "passed",
            "controls": ["A", "Back", "Select action", "tap during locked trick"],
        },
    )
