"""System-hub navigation and return journey."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label, selected_labels


def _select_home_item(driver: MagiKDriver, label: str) -> None:
    for step in range(16):
        if label in selected_labels(driver):
            return
        driver.hat(f"find-{label.lower()}-{step}", 1, 0)
    raise AssertionError(f"home item {label!r} never became selected")


def test_system_hub_opens_from_home_and_returns(magik: MagiKDriver) -> None:
    _select_home_item(magik, "Arcade")
    magik.button("open-arcade-system-hub", Button.A)
    hub = element_with_label(magik, "Arcade")
    assert hub.accessible_item_selected

    magik.button("return-to-home", Button.B)
    launcher = element_with_label(magik, "MiSTer MagiK Launcher")
    assert launcher.accessible_enabled
