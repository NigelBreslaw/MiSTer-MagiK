"""The two probe scenarios: explicit assertions plus retained measurements."""

from __future__ import annotations

import time
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

from magik2.client import NativeAgent
from magik2.testing import one_element, screenshot


def smoke(
    application: Any, screenshot_path: Path, expected_sha256: str
) -> Mapping[str, object]:
    build = one_element(application, "build-label")
    if not build.is_valid:
        raise AssertionError("build label is not valid")
    _expect_value(build, expected_sha256)
    counter = one_element(application, "counter")
    _expect_value(counter, "0")
    one_element(application, "increment").invoke_accessible_default_action()
    _wait(lambda: _value_is(counter, "1"), "counter did not increment")
    one_element(application, "reset").invoke_accessible_default_action()
    _wait(lambda: _value_is(counter, "0"), "counter did not reset")
    one_element(application, "details-toggle").invoke_accessible_default_action()
    _wait(lambda: _exists(application, "details-panel"), "details panel did not open")
    one_element(application, "details-toggle").invoke_accessible_default_action()
    _wait(
        lambda: not _exists(application, "details-panel"), "details panel did not close"
    )
    screenshot(application, screenshot_path)
    return {"build_label": build.accessible_label, "screenshot": screenshot_path.name}


def motion(
    application: Any,
    agent: NativeAgent,
    *,
    instrumented: bool = False,
    sleep: Callable[[float], None] = time.sleep,
) -> Mapping[str, object]:
    state = one_element(application, "motion-state")
    if state.accessible_value not in {"idle", "complete"}:
        raise AssertionError("motion workload is already running")
    one_element(application, "start-motion").invoke_accessible_default_action()
    _wait(lambda: _value_is(state, "running"), "motion workload did not start")
    # The app chooses both measurement boundaries using its monotonic clock.
    # Do not inspect accessibility or capture screenshots during the window.
    seconds = 10 if instrumented else 5
    sleep(2 + seconds + 0.3)
    _wait(
        lambda: _value_is(state, "complete"),
        "motion workload did not complete",
        timeout=5,
    )
    metrics = agent.metrics()
    if metrics.get("sha256") != agent.expected_sha256:
        raise AssertionError("metrics belong to a different running artifact")
    window = validate_window(
        metrics.get("window"), instrumented=instrumented, seconds=seconds
    )
    return {
        **window,
        "sha256": agent.expected_sha256,
        "pid": metrics.get("pid"),
        "warmup_seconds": 2,
    }


def validate_window(
    value: object, *, instrumented: bool, seconds: int
) -> dict[str, object]:
    if not isinstance(value, dict):
        raise AssertionError("device returned no completed measurement window")
    names = (
        "start_ms",
        "end_ms",
        "elapsed_ms",
        "width",
        "height",
        "presentations",
        "render_us_total",
        "render_to_present_us_total",
        "physical_latch_posts",
        "physical_latch_flips",
        "physical_drops",
        "latch_rejections",
    )
    if not all(type(value.get(name)) is int and value[name] >= 0 for name in names):
        raise AssertionError("incomplete or invalid device measurement")
    if (
        value["instrumented"] is not instrumented
        or not seconds * 1000 <= value["elapsed_ms"] <= (seconds + 1) * 1000
    ):
        raise AssertionError("wrong measurement duration or instrumentation")
    if value["end_ms"] - value["start_ms"] != value["elapsed_ms"]:
        raise AssertionError("inconsistent device timing boundaries")
    if value.get("evidence_error") or not value.get("drop_baseline_available"):
        raise AssertionError("required hardware evidence is unavailable")
    if (
        value["presentations"] == 0
        or value["physical_latch_posts"] != value["presentations"]
        or value["physical_latch_flips"] != value["presentations"]
        or value["physical_drops"]
        or value["latch_rejections"]
    ):
        raise AssertionError(
            "physical presentation failed: drops, rejections, or unmatched latches"
        )
    return {**value, "physical_evidence_valid": True}


def _expect_value(element: Any, expected: str) -> None:
    if not _value_is(element, expected):
        raise AssertionError(
            f"expected accessibility value {expected!r}, got {element.accessible_value!r}"
        )


def _value_is(element: Any, expected: str) -> bool:
    return element.accessible_value == expected


def _exists(application: Any, label: str) -> bool:
    try:
        one_element(application, label)
    except AssertionError:
        return False
    return True


def _wait(predicate: Callable[[], bool], failure: str, timeout: float = 3) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise AssertionError(failure)
        time.sleep(0.02)


def launcher_smoke(application, screenshot_path, expected_sha256):
    window = application.first_window
    if (
        window is None
        or window.root_element.accessible_label != "MiSTer MagiK Launcher"
    ):
        raise AssertionError("real launcher window is unavailable")
    errors = [
        element.accessible_description
        for element in window.root_element.query_descendants()
        .match_inherits("Rectangle")
        .find_all()
        if element.accessible_label == "Input error"
    ]
    if errors:
        raise AssertionError("launcher input unavailable: " + "; ".join(errors))
    screenshot(application, screenshot_path)
    return {"sha256": expected_sha256, "screenshot": screenshot_path.name}


def launcher_idle(application, agent, *, instrumented=False):
    """Measure the real launcher's ordinary idle loop; no synthetic FPS target."""
    if application.first_window is None:
        raise AssertionError("real launcher window is unavailable")
    agent._successful("measure")
    seconds = 10 if instrumented else 5
    time.sleep(2 + seconds + 0.4)
    metrics = agent.metrics()
    if metrics.get("sha256") != agent.expected_sha256:
        raise AssertionError("metrics belong to another application")
    window = metrics.get("window")
    if not isinstance(window, dict) or window.get("instrumented") is not instrumented:
        raise AssertionError("real launcher returned no matching measurement window")
    if not seconds * 1000 <= window.get("elapsed_ms", 0) <= (seconds + 1) * 1000:
        raise AssertionError("real launcher measurement duration is invalid")
    if window.get("evidence_error"):
        raise AssertionError(window["evidence_error"])
    return {
        **window,
        "workload": "launcher-idle",
        "sha256": agent.expected_sha256,
        "pid": metrics.get("pid"),
        "warmup_seconds": 2,
    }


def validate_development_paths(context):
    if not isinstance(context, dict):
        raise AssertionError("application did not report its runtime paths")
    root = Path("/media/fat/mister-magik-dev")
    if (
        context.get("data_root") != str(root)
        or context.get("main") != "/media/fat/MiSTer_MagiKDev"
    ):
        raise AssertionError(f"wrong development layout: {context}")
    for name in (
        "settings",
        "controllers",
        "catalog",
        "library",
        "user_state",
        "assets",
    ):
        if not isinstance(context.get(name), str) or not Path(
            context[name]
        ).is_relative_to(root):
            raise AssertionError(
                f"{name} is outside the development layout: {context.get(name)}"
            )
    return context
