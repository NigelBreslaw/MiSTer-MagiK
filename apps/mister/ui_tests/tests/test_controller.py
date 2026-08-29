"""Launcher controller-screen journey using the authenticated logical input path."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label, wait_for_label


def test_controller_screen_is_accessible_and_returns(controller: MagiKDriver) -> None:
    panel = element_with_label(controller, "Controller test")
    assert panel.accessible_enabled
    state = element_with_label(controller, "Controller input state")
    assert state.accessible_enabled

    controller.button("return-to-home", Button.B)
    wait_for_label(controller, "MiSTer MagiK Launcher")
