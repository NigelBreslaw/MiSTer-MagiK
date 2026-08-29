"""Screensaver and reduced-motion settings journeys."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import (
    element_with_label,
    open_settings,
    selected_element_with_label,
)


def test_screensaver_and_reduce_motion_settings(magik: MagiKDriver) -> None:
    open_settings(magik)
    for step in range(2):
        magik.hat(f"select-screensaver-{step}", 0, 1)
    magik.button("open-screensaver-settings", Button.A)
    element_with_label(magik, "Screensaver settings")

    enabled = selected_element_with_label(magik, "Screensaver enabled")
    before = enabled.accessible_description
    magik.button("toggle-screensaver", Button.A)
    after = selected_element_with_label(
        magik, "Screensaver enabled"
    ).accessible_description
    assert before != after

    magik.button("return-to-settings", Button.B)
    magik.hat("select-reduce-motion", 0, 1)
    reduce_motion = selected_element_with_label(magik, "Reduce motion")
    before = reduce_motion.accessible_description
    magik.button("toggle-reduce-motion", Button.A)
    assert (
        selected_element_with_label(magik, "Reduce motion").accessible_description
        != before
    )
