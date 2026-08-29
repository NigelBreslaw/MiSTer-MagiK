"""Smoke journeys for menu effects while keeping device mutations sandboxed."""

from __future__ import annotations

from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label
from apps.mister.ui_tests.uinput_joystick import Button
from apps.mister.ui_tests.uinput_keyboard import Key


def test_exit_and_rebuild_are_confirmation_only(magik: MagiKDriver) -> None:
    magik.key("open-settings", Key.F12)
    for step in range(4):
        magik.hat(f"select-exit-{step}", 0, 1)
    magik.button("open-exit-confirmation", Button.A)
    confirmation = element_with_label(magik, "Confirmation")
    assert confirmation.accessible_description
    magik.button("cancel-exit", Button.B)

    magik.hat("select-rebuild", 0, 1)
    magik.button("open-rebuild-confirmation", Button.A)
    element_with_label(magik, "Confirmation")
    magik.button("cancel-rebuild", Button.B)
