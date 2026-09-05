"""The two probe scenarios: explicit assertions plus retained measurements."""

from __future__ import annotations

import time
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

from .client import NativeAgent
from .testing import one_element, screenshot


def smoke(application: Any, screenshot_path: Path) -> Mapping[str, object]:
    build = one_element(application, "build-label")
    if not build.is_valid:
        raise AssertionError("build label is not valid")
    counter = one_element(application, "counter")
    _expect_value(counter, "0")
    one_element(application, "increment").invoke_accessible_default_action()
    _wait(lambda: _value_is(counter, "1"), "counter did not increment")
    one_element(application, "reset").invoke_accessible_default_action()
    _wait(lambda: _value_is(counter, "0"), "counter did not reset")
    one_element(application, "details-toggle").invoke_accessible_default_action()
    _wait(lambda: _exists(application, "details-panel"), "details panel did not open")
    one_element(application, "details-toggle").invoke_accessible_default_action()
    _wait(lambda: not _exists(application, "details-panel"), "details panel did not close")
    screenshot(application, screenshot_path)
    return {"build_label": build.accessible_label, "screenshot": screenshot_path.name}


def motion(
    application: Any,
    agent: NativeAgent,
    *,
    sleep: Callable[[float], None] = time.sleep,
) -> Mapping[str, object]:
    state = one_element(application, "motion-state")
    _expect_value(state, "idle")
    one_element(application, "start-motion").invoke_accessible_default_action()
    _wait(lambda: _value_is(state, "running"), "motion workload did not start")
    sleep(2)
    samples = []
    for _ in range(5):
        sleep(5)
        samples.append(_measurement(agent.metrics()))
    _wait(lambda: _value_is(state, "complete"), "motion workload did not complete", timeout=10)
    final = _measurement(agent.metrics())
    evidence_valid = final["vsync_misses"] == 0 and final["vsync_hits"] >= final["presentations"]
    return {
        "warmup_seconds": 2,
        "sample_seconds": 5,
        "samples": samples,
        "final": final,
        # The direct fb0 presenter exposes timing and vsync evidence, but it has
        # no scanout-latch acknowledgement. Keep that limitation explicit until
        # the hardware presenter adds the required physical confirmation.
        "physical_evidence_valid": False,
        "vsync_evidence_consistent": evidence_valid,
    }


def _measurement(metrics: Mapping[str, object]) -> dict[str, int]:
    names = ("elapsed_ms", "presentations", "render_us_total", "last_render_us", "vsync_hits", "vsync_misses")
    values = {name: metrics.get(name) for name in names}
    if not all(type(value) is int for value in values.values()):
        raise AssertionError("agent returned incomplete device timing metrics")
    return {name: value for name, value in values.items() if isinstance(value, int)}


def _expect_value(element: Any, expected: str) -> None:
    if not _value_is(element, expected):
        raise AssertionError(f"expected accessibility value {expected!r}, got {element.accessible_value!r}")


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
