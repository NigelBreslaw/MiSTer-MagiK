"""Startup and home navigation journeys."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import (
    element_with_label,
    selected_labels,
    wait_for_label,
)


def test_startup_exposes_launcher_and_opens_arcade(magik: MagiKDriver) -> None:
    launcher = element_with_label(magik, "MiSTer MagiK Launcher")
    assert launcher.accessible_enabled
    assert selected_labels(magik) == ("Arcade",)

    magik.button("open-selected-home-item", Button.A)
    wait_for_label(magik, "Arcade games")
