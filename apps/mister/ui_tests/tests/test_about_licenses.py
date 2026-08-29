"""About and open-source license journeys."""

from __future__ import annotations

from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label
from apps.mister.ui_tests.uinput_joystick import Button
from apps.mister.ui_tests.uinput_keyboard import Key


def test_about_and_license_text_are_accessible(magik: MagiKDriver) -> None:
    magik.key("open-settings", Key.F12)
    for step in range(6):
        magik.hat(f"select-about-{step}", 0, 1)
    magik.button("open-about", Button.A)
    element_with_label(magik, "About")

    magik.hat("select-licenses", 0, 1)
    magik.button("open-licenses", Button.A)
    element_with_label(magik, "Licenses")
    magik.button("open-first-license", Button.A)
    license_text = element_with_label(magik, "License text")
    assert license_text.accessible_description


def test_controller_scene_reports_real_button_state(controller: MagiKDriver) -> None:
    panel = element_with_label(controller, "Controller test")
    assert panel.accessible_enabled
    controller.button("press-a", Button.A)
    state = element_with_label(controller, "Controller input state")
    assert "A" in state.accessible_description
