"""The two probe scenarios: explicit assertions plus retained measurements."""

from __future__ import annotations

import json
import os
import re
import time
from magik.measurement_contract import has_native_card_helpers, measurement_metrics
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

from magik.client import AgentError, NativeAgent
from magik.testing import one_element, screenshot


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
    metrics = measurement_metrics(agent)
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
        "dropped_frames",
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
        or value["dropped_frames"]
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


def _settings_element(application):
    window = application.first_window
    if window is None:
        return None
    return next(
        (
            element
            for element in window.root_element.query_descendants()
            .match_inherits("Rectangle")
            .find_all()
            if element.accessible_label == "Settings"
            and element.accessible_role.name == "Main"
        ),
        None,
    )


def _settings_open(application):
    return _settings_element(application) is not None


def _settings_ready(application):
    element = _settings_element(application)
    return element is not None and element.accessible_description == "Ready"


def _open_settings_card(application):
    _press_key(application, "\uf729")  # Slint Key.Home
    _wait(lambda: not _settings_open(application), "Home did not close Settings")
    # Home can preserve the selected card, and a key during spring settling
    # can be ignored. Observe the native card model instead of counting keys.
    for _ in range(12):
        if "Settings" in _selected_labels(application):
            break
        _press_key(application, "\uf703")  # Slint Key.RightArrow
        time.sleep(1)  # Allow the carousel's spring to settle before retrying.
    else:
        raise AssertionError("Settings card was not selectable within 12 attempts")
    activated_at = time.monotonic()
    _press_key(application, "\n")  # Slint Key.Return
    return activated_at


def launcher_navigation(application, screenshot_path, agent=None):
    """One bounded UI journey; response times include host RPC and polling."""
    started = time.monotonic()
    activated_at = _open_settings_card(application)
    transition_observed = False
    opening_capture = None
    capture_attempted = False
    opening_capture_unavailable = None

    def settled():
        nonlocal \
            transition_observed, \
            opening_capture, \
            capture_attempted, \
            opening_capture_unavailable
        element = _settings_element(application)
        if element is None:
            return False
        if element.accessible_description == "Transitioning":
            transition_observed = True
            if agent is not None and not capture_attempted:
                capture_attempted = True
                # One functional capture outside any cadence measurement. Allow
                # the first source frame to advance into the card/cog handoff.
                time.sleep(0.12)
                from magik.capture import capture_png

                try:
                    fields, pixels = agent.capture_framebuffer()
                except AgentError as error:
                    if not str(error).startswith("capture-frame-changed:"):
                        raise
                    opening_capture_unavailable = (
                        "scanout changed during animation; no retry"
                    )
                else:
                    png, metadata = capture_png(fields, pixels, "raw")
                    path = screenshot_path.with_name("settings-opening-native.png")
                    path.write_bytes(png)
                    path.with_suffix(".json").write_text(
                        json.dumps(metadata, indent=2) + "\n"
                    )
                    opening_capture = path.name
        return element.accessible_description == "Ready"

    try:
        _wait(settled, "Settings transition did not settle")
        opened_ms = round((time.monotonic() - started) * 1000, 2)
        activation_to_ready_ms = round((time.monotonic() - activated_at) * 1000, 2)
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
        "activation_to_ready_ms": activation_to_ready_ms,
        "transition_observed": transition_observed,
        "opening_capture": opening_capture,
        "opening_capture_unavailable": opening_capture_unavailable,
        "back_response_ms": round((time.monotonic() - returned) * 1000, 2),
        "timing_source": "host RPC and accessibility polling; not frame latency",
        "screenshot": screenshot_path.name,
    }


def launcher_idle(application, agent, *, instrumented=False):
    """Measure the real launcher's ordinary idle loop; no synthetic FPS target."""
    if application.first_window is None:
        raise AssertionError("real launcher window is unavailable")
    previous = measurement_metrics(agent).get("window")
    agent._successful("measure")
    seconds = 10 if instrumented else 5
    time.sleep(2 + seconds + 0.4)
    metrics = measurement_metrics(agent)
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


# Six right taps 250 ms apart, then right held for ten seconds; the window
# keeps one further second for the carousel to settle after release.
TAPS_THEN_HOLD = {"taps": 6, "tap_interval_ms": 250, "hold_ms": 10_000}
TAPS_THEN_HOLD_WINDOW_MS = 6 * 250 + 10_000 + 1_000
# Bounded wait for a window that started late because the request arrived idle.
WINDOW_PICKUP_GRACE_SECONDS = 3


def _completed_window_metrics(agent, sleep):
    """Read metrics, waiting boundedly for a window that started late.

    An idle launcher can service the request up to about a second late, so the
    device-timed window may still be running at the nominal deadline.
    """
    metrics = measurement_metrics(agent)
    deadline = time.monotonic() + WINDOW_PICKUP_GRACE_SECONDS
    while metrics.get("window") is None and time.monotonic() < deadline:
        sleep(0.25)
        metrics = measurement_metrics(agent)
    return metrics


def launcher_motion(
    application,
    agent,
    *,
    instrumented: bool = False,
    align_rollover: bool = False,
    force_fallback: bool = False,
    held_direction: bool = False,
    taps_then_hold: bool = False,
    starting_card: str | None = None,
    raw_metrics_path: Path | None = None,
    sleep: Callable[[float], None] = time.sleep,
):
    """Measure continuous card-carousel navigation on the real launcher."""
    if taps_then_hold and (held_direction or instrumented):
        raise ValueError("taps_then_hold is a standalone uninstrumented workload")
    if application.first_window is None:
        raise AssertionError("real launcher window is unavailable")
    _press_key(application, "\uf729")  # Slint Key.Home
    _wait(lambda: not _settings_open(application), "Home did not close Settings")

    if held_direction or taps_then_hold:
        # Home preserves an in-progress card spring. A new press can be rejected
        # until it settles; keep this pause outside the measured hold window.
        sleep(1)
    if starting_card is not None:
        from magik.animation_benchmark import _focus

        _focus(application, "^" + re.escape(starting_card) + "$")
        sleep(1)
    previous = measurement_metrics(agent).get("window")
    request = {
        "launcher_clock": "rollover" if align_rollover else "fixed",
        "launcher_fallback": force_fallback,
        "duration_ms": 8_000 if held_direction else 5_000,
        "launcher_hold": held_direction,
    }
    if taps_then_hold:
        # Device-timed from the window start: taps, a hold, then the settle.
        request["launcher_sequence"] = TAPS_THEN_HOLD
        request["duration_ms"] = TAPS_THEN_HOLD_WINDOW_MS
    agent._successful("measure", request)
    seconds = (
        TAPS_THEN_HOLD_WINDOW_MS / 1000
        if taps_then_hold
        else 10
        if instrumented
        else (8 if held_direction else 5)
    )
    interval_seconds = 0.25
    deadline = time.monotonic() + 2 + seconds + 0.4
    input_events = 0
    if taps_then_hold:
        # Taps and the hold are device-timed; host RPC jitter cannot shift them.
        # Release only cancels: the window must complete before it is sent.
        try:
            input_events += TAPS_THEN_HOLD["taps"] + 1
            sleep(2 + seconds + 0.4)
            metrics = _completed_window_metrics(agent, sleep)
        finally:
            agent._successful("measure", {"launcher_hold": "release"})
    elif held_direction:
        # The device feeds a bounded press/release through the real input router.
        # The development keyboard bridge emits taps, so it cannot sustain holds.
        try:
            input_events += 1
            sleep(2 + seconds + 0.4)
            metrics = _completed_window_metrics(agent, sleep)
        finally:
            agent._successful("measure", {"launcher_hold": "release"})
            input_events += 1
    else:
        while time.monotonic() < deadline:
            direction = "\uf703" if (input_events // 5) % 2 == 0 else "\uf702"
            _press_key(application, direction)
            input_events += 1
            sleep(interval_seconds)
        metrics = _completed_window_metrics(agent, sleep)

    if raw_metrics_path is not None:
        raw_metrics_path.write_text(json.dumps(metrics, indent=2) + "\n")
    if metrics.get("sha256") != agent.expected_sha256:
        raise AssertionError("metrics belong to another application")
    window = metrics.get("window")
    if not isinstance(window, dict) or window.get("instrumented") is not instrumented:
        raise AssertionError(
            "real launcher returned no matching measurement window "
            f"(device elapsed_ms={metrics.get('elapsed_ms')}, window={window!r})"
        )
    if not seconds * 1000 <= window.get("elapsed_ms", 0) <= (seconds + 1) * 1000:
        raise AssertionError("real launcher measurement duration is invalid")
    if isinstance(previous, dict) and window.get("start_ms", -1) <= previous.get(
        "end_ms", -1
    ):
        raise AssertionError("measurement returned a previous window")
    if window.get("evidence_error"):
        raise AssertionError(window["evidence_error"])
    if window.get("presentations", 0) <= 0:
        raise AssertionError("card navigation produced no measured presentations")
    if window.get("forced_clock_changes") != int(align_rollover):
        raise AssertionError(
            "measurement did not observe the requested synthetic clock change"
        )
    if instrumented:
        unique = window.get("card_delivered_frames", 0)
        dropped = window.get("dropped_frames", 0)
        if unique + dropped + window.get("card_synchronous_presentations", 0) != window[
            "presentations"
        ] + window.get("owned_refresh_dropped_frames", 0):
            raise AssertionError(
                "delivered and dropped frame counts do not cover animation refreshes"
            )
        if has_native_card_helpers(window):
            if window.get("card_producer_total_us", 0) <= 0:
                raise AssertionError(
                    "instrumented card motion recorded no producer work"
                )
            if window.get("card_hidden_copy_us", 0) <= 0:
                raise AssertionError(
                    "instrumented card motion recorded no hidden-slot copy work"
                )
        else:
            if (
                window.get("process_cpu_us", 0) <= 0
                or window.get("transfer_us_total", 0) <= 0
            ):
                raise AssertionError(
                    "instrumented compositor motion recorded no CPU/copy work"
                )
    if (
        held_direction
        and window.get("card_continuous_presentations") != window["presentations"]
    ):
        raise AssertionError(
            "held carousel did not remain in continuous motion for the whole window"
        )
    if force_fallback and window.get("card_fallback_copies", 0) == 0:
        raise AssertionError("forced fallback did not execute")
    return {
        **window,
        "workload": (
            "launcher-card-motion-taps-then-hold"
            if taps_then_hold
            else "launcher-card-motion-held"
            if held_direction
            else "launcher-card-motion-rollover"
            if align_rollover
            else "launcher-card-motion"
        ),
        "sha256": agent.expected_sha256,
        "pid": metrics.get("pid"),
        "warmup_seconds": 2,
        "input_events": input_events,
        "input_interval_ms": (
            TAPS_THEN_HOLD["tap_interval_ms"]
            if taps_then_hold
            else None
            if held_direction
            else int(interval_seconds * 1000)
        ),
        "launcher_sequence": TAPS_THEN_HOLD if taps_then_hold else None,
        "held_direction": "right" if held_direction else None,
        "starting_card": starting_card,
        "held_measurement_ms": seconds * 1000 if held_direction else 0,
    }


def _screensaver_window_ms() -> int:
    """Window length, overridable for longer runs; the device accepts 1-45 s."""
    text = os.environ.get("MAGIK_SCREENSAVER_WINDOW_MS", "10000")
    if not text.isdecimal() or not 1_000 <= int(text) <= 45_000:
        raise SystemExit(
            f"MAGIK_SCREENSAVER_WINDOW_MS must be 1000-45000, not {text!r}"
        )
    return int(text)


SCREENSAVER_WINDOW_MS = _screensaver_window_ms()


def launcher_screensaver(application, agent, *, sleep=time.sleep):
    """Measure the screensaver, started on request and held for the window."""
    if application.first_window is None:
        raise AssertionError("real launcher window is unavailable")
    _press_key(application, "\uf729")  # Slint Key.Home
    _wait(lambda: not _settings_open(application), "Home did not close Settings")
    sleep(1)
    previous = measurement_metrics(agent).get("window")
    seconds = SCREENSAVER_WINDOW_MS / 1000
    agent._successful(
        "measure",
        {
            "launcher_clock": "fixed",
            "launcher_fallback": False,
            "launcher_hold": False,
            "launcher_screensaver": True,
            "duration_ms": SCREENSAVER_WINDOW_MS,
        },
    )
    try:
        sleep(2 + seconds + 0.4)
        metrics = _completed_window_metrics(agent, sleep)
    finally:
        # Cancel the request, then wake the launcher like a user would.
        agent._successful("measure", {"launcher_hold": "release"})
        _press_key(application, "\uf729")
    if metrics.get("sha256") != agent.expected_sha256:
        raise AssertionError("metrics belong to another application")
    window = metrics.get("window")
    if not isinstance(window, dict) or window.get("instrumented") is not False:
        raise AssertionError(
            "real launcher returned no matching measurement window "
            f"(device elapsed_ms={metrics.get('elapsed_ms')}, window={window!r})"
        )
    if not seconds * 1000 <= window.get("elapsed_ms", 0) <= (seconds + 1) * 1000:
        raise AssertionError("real launcher measurement duration is invalid")
    if isinstance(previous, dict) and window.get("start_ms", -1) <= previous.get(
        "end_ms", -1
    ):
        raise AssertionError("measurement returned a previous window")
    if window.get("evidence_error"):
        raise AssertionError(window["evidence_error"])
    if window.get("presentations", 0) <= 0:
        raise AssertionError("screensaver produced no measured presentations")
    if window.get("screensaver_presentations") != window["presentations"]:
        raise AssertionError("screensaver was not shown for the whole window")
    return {
        **window,
        "workload": "launcher-screensaver",
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


def _menu_ready(application):
    if _settings_open(application):
        return _settings_ready(application)
    if _exists(application, "Collections"):
        return one_element(application, "Collections").accessible_description == "Ready"
    return False


def _focus_label(application, label, key, limit):
    """Move through a bounded menu, observing each acknowledged focus change."""
    for _ in range(limit):
        _wait(lambda: _menu_ready(application), "menu transition did not settle")
        before = _selected_labels(application)
        if label in before:
            return
        _press_key(application, key)
        _wait(
            lambda before=before: _selected_labels(application) != before,
            "menu focus did not move",
        )
    if label not in _selected_labels(application):
        raise AssertionError(f"{label!r} was not selectable within {limit} steps")


def _open_arcade_games(application):
    """Open the Arcade game list from the Arcade card.

    Some routes (the CRT) show the Arcade hub first, which moves the selection
    off the Arcade card without showing the list; the list then needs a second
    Enter.
    """
    before = _selected_labels(application)
    _press_key(application, "\n")
    _wait(
        lambda: (
            _exists(application, "Arcade games")
            or _selected_labels(application) != before
        ),
        "Arcade did not open",
        timeout=10,
    )
    if not _exists(application, "Arcade games"):
        _press_key(application, "\n")
    _wait(
        lambda: _exists(application, "Arcade games"),
        "Arcade catalog did not open",
        timeout=10,
    )


def _hold_in_arcade_list(agent, direction, seconds, sleep):
    """Hold a direction through one device-timed window and return its metrics."""
    agent._successful(
        "measure",
        {
            "launcher_clock": "fixed",
            "launcher_hold": True,
            "launcher_hold_direction": direction,
            "duration_ms": seconds * 1000,
        },
    )
    try:
        sleep(2 + seconds + 0.4)
        return _completed_window_metrics(agent, sleep)
    finally:
        agent._successful("measure", {"launcher_hold": "release"})


def launcher_arcade_scroll(
    application,
    agent,
    *,
    instrumented: bool = False,
    raw_metrics_path: Path | None = None,
    sleep: Callable[[float], None] = time.sleep,
):
    """Hold Down in the Arcade list for a measured window, then return Home.

    The window records presentations, drops and render timings like the card
    carousel's. The selection ends deep in the list; it is not restored.
    """
    if application.first_window is None:
        raise AssertionError("real launcher window is unavailable")
    _press_key(application, "\uf729")
    _wait(lambda: not _settings_open(application), "Home did not close Settings")
    sleep(1)
    _focus_label(application, "Arcade", "\uf703", 16)
    _open_arcade_games(application)
    sleep(1)
    # HDMI portrait draws the list without exposing its selection.
    started_at = one_element(application, "Arcade games").accessible_value
    started_at = int(started_at) if started_at.isdigit() else None
    previous = measurement_metrics(agent).get("window")
    seconds = 10 if instrumented else 8
    metrics = _hold_in_arcade_list(agent, "down", seconds, sleep)
    try:
        # Scroll back so the list is where the run found it.
        _hold_in_arcade_list(agent, "up", seconds, sleep)
        current = one_element(application, "Arcade games").accessible_value
        if started_at is not None and int(current) > started_at:
            raise AssertionError("the Arcade list did not scroll back to its start")
    finally:
        _press_key(application, "\uf729")
    _wait(
        lambda: not _exists(application, "Arcade games"),
        "Home did not close the catalog",
    )
    if raw_metrics_path is not None:
        raw_metrics_path.write_text(json.dumps(metrics, indent=2) + "\n")
    if metrics.get("sha256") != agent.expected_sha256:
        raise AssertionError("metrics belong to another application")
    window = metrics.get("window")
    if not isinstance(window, dict) or window.get("instrumented") is not instrumented:
        raise AssertionError(
            "real launcher returned no matching measurement window "
            f"(device elapsed_ms={metrics.get('elapsed_ms')}, window={window!r})"
        )
    if not seconds * 1000 <= window.get("elapsed_ms", 0) <= (seconds + 1) * 1000:
        raise AssertionError("arcade scroll measurement duration is invalid")
    if isinstance(previous, dict) and window.get("start_ms", -1) <= previous.get(
        "end_ms", -1
    ):
        raise AssertionError("measurement returned a previous window")
    if window.get("evidence_error"):
        raise AssertionError(window["evidence_error"])
    if window.get("presentations", 0) <= 0:
        raise AssertionError("arcade scrolling produced no measured presentations")
    return {**window, "workload": "arcade-scroll-down"}


def launcher_catalog(application, screenshot_path):
    """Use the installed Dev catalog; do not launch a core or mutate the catalog."""
    _press_key(application, "\uf729")
    _focus_label(application, "Arcade", "\uf703", 16)
    started = time.monotonic()
    before = None
    reverse = None
    try:
        _open_arcade_games(application)
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
                    lambda: (
                        one_element(application, "Arcade games").accessible_value
                        == before
                    ),
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
    _open_settings_card(application)
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
