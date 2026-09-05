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


def _press_key(application, text):
    from slint_testing import KeyPressedEvent, KeyReleasedEvent

    window = application.first_window
    if window is None:
        raise AssertionError("real launcher window is unavailable")
    window.dispatch_event(KeyPressedEvent(text))
    window.dispatch_event(KeyReleasedEvent(text))


def _settings_open(application):
    return any(
        element.accessible_label == "Settings"
        and element.accessible_role.name == "Main"
        for element in application.first_window.root_element.query_descendants()
        .match_inherits("Rectangle")
        .find_all()
    )


def launcher_navigation(application, screenshot_path):
    """One bounded UI journey; response times include host RPC and polling."""
    _press_key(application, "\uf729")  # Slint Key.Home
    _wait(lambda: not _settings_open(application), "Home did not close Settings")
    started = time.monotonic()
    _press_key(application, "\uf700")  # Slint Key.UpArrow: focus Settings
    _press_key(application, "\n")  # Slint Key.Return
    try:
        _wait(lambda: _settings_open(application), "Settings did not open")
        opened_ms = round((time.monotonic() - started) * 1000, 2)
        screenshot(application, screenshot_path)
    finally:
        # Return without changing a setting, including after screenshot failure.
        returned = time.monotonic()
        if _settings_open(application):
            _press_key(application, "\x1b")  # Slint Key.Escape
    _wait(lambda: not _settings_open(application), "Settings did not close")
    return {
        "workload": "home-settings-home",
        "open_response_ms": opened_ms,
        "back_response_ms": round((time.monotonic() - returned) * 1000, 2),
        "timing_source": "host RPC and accessibility polling; not frame latency",
        "screenshot": screenshot_path.name,
    }


def launcher_idle(application, agent, *, instrumented=False):
    """Measure the real launcher's ordinary idle loop; no synthetic FPS target."""
    if application.first_window is None:
        raise AssertionError("real launcher window is unavailable")
    previous = agent.metrics().get("window")
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
    if isinstance(previous, dict) and window.get("start_ms", -1) <= previous.get(
        "end_ms", -1
    ):
        raise AssertionError("measurement returned a previous window")
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
        value = context.get(name)
        if (
            not isinstance(value, str)
            or ".." in Path(value).parts
            or not Path(value).is_relative_to(root)
        ):
            raise AssertionError(
                f"{name} is outside the development layout: {context.get(name)}"
            )
    return context


def _selected_labels(application):
    return [
        element.accessible_label
        for element in application.first_window.root_element.query_descendants()
        .match_inherits("Rectangle")
        .find_all()
        if element.accessible_item_selected
    ]


def _focus_label(application, label, key, limit):
    """Move through a bounded menu, observing each acknowledged focus change."""
    for _ in range(limit):
        before = _selected_labels(application)
        if label in before:
            return
        _press_key(application, key)
        _wait(
            lambda: _selected_labels(application) != before, "menu focus did not move"
        )
    if label not in _selected_labels(application):
        raise AssertionError(f"{label!r} was not selectable within {limit} steps")


def launcher_catalog(application, screenshot_path):
    """Use the installed Dev catalog; do not launch a core or mutate the catalog."""
    _press_key(application, "\uf729")
    _focus_label(application, "Arcade", "\uf703", 16)
    started = time.monotonic()
    before = None
    reverse = None
    try:
        _press_key(application, "\n")
        _wait(
            lambda: _exists(application, "Arcade games"),
            "Arcade catalog did not open",
            timeout=10,
        )
        games = one_element(application, "Arcade games")
        if not games.accessible_enabled:
            raise AssertionError("Arcade catalog is disabled")
        # Rust paints the rows; the list exposes the one-based selection.
        _wait(
            lambda: bool(one_element(application, "Arcade games").accessible_value),
            "Arcade catalog has no active game",
            timeout=10,
        )
        count = games.accessible_description
        if (
            not count.removesuffix(" games").isdigit()
            or int(count.removesuffix(" games")) < 2
        ):
            raise AssertionError(
                f"journey requires at least two Dev Arcade games; found {count!r}"
            )
        before = one_element(application, "Arcade games").accessible_value
        if not before.isdigit() or not 1 <= int(before) <= int(
            count.removesuffix(" games")
        ):
            raise AssertionError(f"invalid catalog selection: {before!r}")
        key, reverse = ("\uf700", "\uf701") if int(before) > 1 else ("\uf701", "\uf700")
        _press_key(application, key)
        _wait(
            lambda: one_element(application, "Arcade games").accessible_value != before,
            f"catalog selection did not move from {before!r}",
        )
        screenshot(application, screenshot_path)
        elapsed_ms = round((time.monotonic() - started) * 1000, 2)
    finally:
        try:
            if reverse is not None:
                current = one_element(application, "Arcade games").accessible_value
                if current != before:
                    _press_key(application, reverse)
                _wait(
                    lambda: one_element(application, "Arcade games").accessible_value
                    == before,
                    "catalog selection was not restored",
                )
        finally:
            _press_key(application, "\uf729")
    _wait(
        lambda: not _exists(application, "Arcade games"),
        "Home did not close the catalog",
    )
    return {
        "workload": "arcade-select-home",
        "response_ms": elapsed_ms,
        "timing_source": "host RPC and accessibility polling; not frame latency",
    }


def launcher_setting(application, screenshot_path):
    """Change one reversible Dev setting and verify restoration even on failure."""
    _press_key(application, "\uf729")
    _press_key(application, "\uf700")
    _press_key(application, "\n")
    original = None
    try:
        _wait(lambda: _settings_open(application), "Settings did not open")
        _focus_label(application, "Reduce motion", "\uf701", 8)
        setting = one_element(application, "Reduce motion")
        original = setting.accessible_description
        if original not in {"On", "Off"}:
            raise AssertionError(f"unknown Reduce motion value: {original!r}")
        started = time.monotonic()
        _press_key(application, "\n")
        expected = "Off" if original == "On" else "On"
        _wait(
            lambda: (
                one_element(application, "Reduce motion").accessible_description
                == expected
            ),
            "Reduce motion did not change",
        )
        screenshot(application, screenshot_path)
        elapsed_ms = round((time.monotonic() - started) * 1000, 2)
    finally:
        # Read back the current state: a failed acknowledgement may still have
        # applied the change. Never blindly replay the toggle during cleanup.
        try:
            if original in {"On", "Off"}:
                current = one_element(
                    application, "Reduce motion"
                ).accessible_description
                if current != original:
                    if current not in {"On", "Off"}:
                        raise AssertionError(
                            "cannot safely restore unknown Reduce motion state"
                        )
                    _focus_label(application, "Reduce motion", "\uf701", 8)
                    _press_key(application, "\n")
                _wait(
                    lambda: (
                        one_element(application, "Reduce motion").accessible_description
                        == original
                    ),
                    "Reduce motion was not restored",
                )
        finally:
            _press_key(application, "\uf729")
    _wait(lambda: not _settings_open(application), "Home did not close Settings")
    return {
        "workload": "reduce-motion-toggle-restore",
        "original": original,
        "restored": True,
        "response_ms": elapsed_ms,
        "timing_source": "host RPC and accessibility polling; not frame latency",
    }
