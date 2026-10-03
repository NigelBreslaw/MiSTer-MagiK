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


def _settled_metrics(metrics, before_ms, deadline):
    """A fresh wall-time sample must report idle; late frame-locked motion may take longer."""
    while True:
        value = metrics()
        assert type(value.get("ui_motion")) is bool, "Missing UI motion evidence"
        assert time.monotonic() < deadline, (
            "Animation did not settle inside the route window"
        )
        if value["elapsed_ms"] >= before_ms + 2_000 and not value["ui_motion"]:
            return value
        time.sleep(0.2)


def animation_roundtrip(
    app, agent, run: Path, repetition: int, *, instrumented=False, frame_evidence="off"
):
    prefix = f"animation-roundtrip-{repetition}"
    _key(app, "\uf729")
    time.sleep(1.2)
    _focus(app, r"^Consoles$")
    agent._successful(
        "measure", {"duration_ms": 45_000, "frame_evidence": frame_evidence}
    )
    requested_at = time.monotonic()
    time.sleep(3.5)

    animation_clock = None

    def metrics():
        nonlocal animation_clock
        value = agent.metrics()
        assert value["sha256"] == agent.expected_sha256
        assert not value.get("evidence_error"), value.get("evidence_error")
        clock = value.get("context", {}).get("animation_clock", {})
        assert clock.get("mode") == "vsync-locked-v1", (
            "Rebuild with the frame-clock benchmark contract"
        )
        assert type(clock.get("period_ns")) is int and clock["period_ns"] > 0
        if animation_clock is None:
            animation_clock = clock
        assert clock == animation_clock, "Animation clock changed during the route"
        return value

    before = metrics()
    assert before.get("window") is None, "No fresh measurement window"
    assert before.get("ui_motion") is False, "Initial selection has not settled"
    rows = []

    def step(name, action, expected=None, menu=None, games=False, browsing=False):
        nonlocal before
        action()
        time.sleep(2)
        after = _settled_metrics(metrics, before["elapsed_ms"], requested_at + 43)
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
        if end.get("window") is not None and (
            not instrumented or "renderer_profile" in end["window"]
        ):
            break
        if time.monotonic() >= requested_at + 55:
            (run / f"{prefix}-incomplete.json").write_text(json.dumps(end, indent=2))
            raise AssertionError("Measurement did not complete")
        time.sleep(0.3)
    window = end["window"]
    assert window["target_duration_ms"] == 45_000
    assert 45_000 <= window["elapsed_ms"] < 46_000
    assert window["start_ms"] <= rows[0]["device_before_ms"]
    assert window["end_ms"] >= rows[-1]["device_after_ms"]
    assert window["instrumented"] is instrumented and not window.get("evidence_error")
    if window.get("context", {}).get("card_helper_ahead") == "native-tricks-v1":
        assert window["helper_ahead_frames"] > 0, (
            "Helper render-ahead was not exercised"
        )
        assert window["helper_ahead_frames"] <= window["card_rendered_frames"]
        assert window["helper_ahead_lead_us_total"] > 0
    if instrumented:
        assert window["renderer_profile"]["worker_frames"] > 0, (
            "Missing helper stage evidence"
        )
        stages = window["renderer_profile"]["stages"]
        assert all(
            stages.get(label, {}).get("calls", 0) > 0
            for label in (
                "flip.geometry-filter",
                "flip.compose",
                "reflection.prepare",
                "flip.reflection",
            )
        ), "Missing renderer stage evidence"
    if frame_evidence != "off":
        evidence = window["frame_evidence"]
        assert evidence["mode"] == frame_evidence
        assert evidence["observed_frames"] > 0
        assert evidence["retention_overflow"] == 0
        assert evidence["clock_brackets"]["samples"], "Missing clock calibration"
        assert (
            sum(
                (frame["observation"]["dropped_frames"] + frame["missing_fresh_pose"])
                for frame in evidence["frames"]
            )
            == window["dropped_frames"]
        )
    if frame_evidence == "phases":
        frames = window["frame_evidence"]["frames"]
        completed = [f for f in frames if f["telemetry_valid"]]
        assert completed and all(f["produced_frame_id"] > 0 for f in completed)
        assert all(
            all(v is not None for v in f["phases"]["cpu_us"]) for f in completed
        ), "Incomplete UI CPU evidence"
        helpers = [
            f["phases"]["helper"] for f in frames if f["phases"]["helper"] is not None
        ]
        assert helpers, "Missing helper job evidence"
        assert all(
            h["dispatched_us"]
            <= h["started_us"]
            <= h["finished_us"]
            <= h["received_us"]
            for h in helpers
        )
    route_drops = sum(row["dropped_frames"] for row in rows)
    assert route_drops == window["dropped_frames"], "Route and window counts differ"
    assert sum(window["dropped_frames_by_workload"].values()) == route_drops
    assert window["moving_cpu_unavailable_intervals"] == 0
    assert window["moving_cpu_us"] > 0 and window["moving_presentations"] > 0
    result = {
        "sha256": agent.expected_sha256,
        "animation_clock": animation_clock,
        "steps": rows,
        "window": window,
    }
    (run / f"{prefix}.json").write_text(json.dumps(result, indent=2))
    return result
