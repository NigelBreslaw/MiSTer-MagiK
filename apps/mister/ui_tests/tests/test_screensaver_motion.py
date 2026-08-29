"""Screensaver and reduced-motion settings journeys."""

from __future__ import annotations

from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_any_label, element_with_label
from apps.mister.ui_tests.uinput_joystick import Button
from apps.mister.ui_tests.uinput_keyboard import Key


def test_screensaver_and_reduce_motion_settings(magik: MagiKDriver) -> None:
    magik.key("open-settings", Key.F12)
    for step in range(2):
        magik.hat(f"select-screensaver-{step}", 0, 1)
    magik.button("open-screensaver-settings", Button.A)
    element_with_label(magik, "Screensaver settings")

    enabled = element_with_any_label(magik, ("Screensaver enabled", "Enabled"))
    before = enabled.accessible_description
    magik.button("toggle-screensaver", Button.A)
    after = element_with_any_label(
        magik, ("Screensaver enabled", "Enabled")
    ).accessible_description
    assert before != after

    magik.button("return-to-settings", Button.B)
    magik.hat("select-reduce-motion", 0, 1)
    reduce_motion = element_with_label(magik, "Reduce motion")
    before = reduce_motion.accessible_description
    magik.button("toggle-reduce-motion", Button.A)
    assert element_with_label(magik, "Reduce motion").accessible_description != before
