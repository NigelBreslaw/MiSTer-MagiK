"""Complete real-launcher animation acceptance workload, within one native lease."""

from __future__ import annotations

import json
import re
import time
from pathlib import Path

from .results import append_event

_FIELDS = (
    "presentations",
    "dropped_frames",
    "card_dropped_frames",
    "physical_latch_posts",
    "physical_latch_flips",
    "latch_rejections",
    "render_us_total",
    "render_to_present_us_total",
    "card_producer_total_us",
    "card_primary_tile_us",
    "card_secondary_tile_us",
    "card_secondary_wait_us",
    "card_hidden_copy_us",
)


def _key(app, value):
    from slint_testing import KeyPressedEvent, KeyReleasedEvent

    app.first_window.dispatch_event(KeyPressedEvent(value))
    app.first_window.dispatch_event(KeyReleasedEvent(value))


def _tree(app):
    root = app.first_window.root_element
    props = [
        e._get_props()
        for e in root.query_descendants().match_inherits("Rectangle").find_all()
    ]
    return {
        "menu": root.accessible_description,
        "selected": [p.accessible_label for p in props if p.accessible_item_selected],
        "elements": [
            {"label": p.accessible_label, "description": p.accessible_description}
            for p in props
            if p.accessible_label
        ],
    }


def _selected(tree, pattern):
    return any(re.search(pattern, str(label), re.I) for label in tree["selected"])


def _focus(app, pattern):
    for _ in range(8):
        tree = _tree(app)
        if _selected(tree, pattern):
            return
        _key(app, "\uf703")
        time.sleep(1)
    raise AssertionError(f"Cannot focus {pattern}: {_tree(app)}")


def animation_roundtrip(app, agent, run: Path, repetition: int):
    prefix = f"animation-roundtrip-{repetition}"
    _key(app, "\uf729")
    time.sleep(1.2)
    _focus(app, r"^Consoles$")
    agent._successful("measure", {"duration_ms": 45_000})
    requested_at = time.monotonic()
    time.sleep(3.5)

    def metrics():
        value = agent.metrics()
        assert value["sha256"] == agent.expected_sha256
        assert not value.get("evidence_error"), value.get("evidence_error")
        return value

    before = metrics()
    assert before.get("window") is None, "No fresh measurement window"
    rows = []

    def step(name, action, expected=None, menu=None, games=False, browsing=False):
        nonlocal before
        action()
        time.sleep(2)
        tree = _tree(app)
        if expected:
            assert _selected(tree, expected), (name, tree)
        if menu:
            assert re.search(menu, str(tree["menu"]), re.I), (name, tree)
        if games:
            assert any(e["label"] == "Arcade games" for e in tree["elements"]), (
                name,
                tree,
            )
        if browsing:
            assert any(
                e == {"label": "Collections", "description": "Ready"}
                for e in tree["elements"]
            ), (name, tree)
        after = metrics()
        assert after["elapsed_ms"] > before["elapsed_ms"], "Stale metrics"
        row = {
            "name": name,
            "device_before_ms": before["elapsed_ms"],
            "device_after_ms": after["elapsed_ms"],
            "tree": tree,
            **{field: after.get(field, 0) - before.get(field, 0) for field in _FIELDS},
        }
        rows.append(row)
        append_event(run, {"phase": "animation-step", "repetition": repetition, **row})
        (run / f"{prefix}-step-{len(rows)}.json").write_text(
            json.dumps(after, indent=2)
        )
        before = after

    step("Root → Consoles", lambda: _key(app, "\n"), menu="Consoles", browsing=True)
    if not _selected(_tree(app), r"^Nintendo$"):
        step(
            "Browse to Nintendo",
            lambda: _focus(app, r"^Nintendo$"),
            expected=r"^Nintendo$",
            browsing=True,
        )
    step("Consoles → Nintendo", lambda: _key(app, "\n"), menu="Nintendo", browsing=True)
    if not _selected(_tree(app), r"SNES|Super Nintendo"):
        step(
            "Browse to SNES",
            lambda: _focus(app, r"SNES|Super Nintendo"),
            expected=r"SNES|Super Nintendo",
            browsing=True,
        )
    step("Nintendo → SNES hub", lambda: _key(app, "\n"), expected=r"^GAMES$")
    step("SNES hub → Games list", lambda: _key(app, "\n"), games=True)
    step(
        "Games list → Nintendo",
        lambda: _key(app, "\x1b"),
        menu="Nintendo",
        browsing=True,
    )
    step(
        "Nintendo → Consoles", lambda: _key(app, "\x1b"), menu="Consoles", browsing=True
    )
    step(
        "Consoles → Root",
        lambda: _key(app, "\x1b"),
        expected=r"^Consoles$",
        browsing=True,
    )
    while True:
        end = metrics()
        if end.get("window") is not None:
            break
        assert time.monotonic() < requested_at + 55, "Measurement did not complete"
        time.sleep(0.3)
    window = end["window"]
    assert window["target_duration_ms"] == 45_000
    assert 45_000 <= window["elapsed_ms"] < 46_000
    assert window["start_ms"] <= rows[0]["device_before_ms"]
    assert window["end_ms"] >= rows[-1]["device_after_ms"]
    assert not window["instrumented"] and not window.get("evidence_error")
    route_drops = sum(row["dropped_frames"] for row in rows)
    assert route_drops == window["dropped_frames"], "Route and window counts differ"
    assert sum(window["dropped_frames_by_workload"].values()) == route_drops
    assert window["moving_cpu_unavailable_intervals"] == 0
    assert window["moving_cpu_us"] > 0 and window["moving_presentations"] > 0
    result = {"sha256": agent.expected_sha256, "steps": rows, "window": window}
    (run / f"{prefix}.json").write_text(json.dumps(result, indent=2))
    return result
