"""System-hub navigation and return journey."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import (
    element_with_label,
    elements_with_label,
    selected_labels,
    selected_element_with_label,
    wait_for_label,
)


def _select_home_item(driver: MagiKDriver, label: str) -> None:
    for step in range(16):
        if label in selected_labels(driver):
            return
        driver.hat(f"find-{label.lower()}-{step}", 1, 0)
    raise AssertionError(f"home item {label!r} never became selected")


def test_system_hub_opens_from_home_and_returns(magik: MagiKDriver) -> None:
    _select_home_item(magik, "Consoles")
    magik.button("open-consoles", Button.A)

    _select_home_item(magik, "Nintendo SNES")
    magik.button("open-snes", Button.A)

    games = selected_element_with_label(magik, "GAMES")
    assert games.accessible_item_selected
    assert games.accessible_description == "1 TITLES"
    recent = elements_with_label(magik, "RECENT")
    assert recent and all(element.accessible_enabled for element in recent)
    favorites = elements_with_label(magik, "FAVORITES")
    assert favorites and all(element.accessible_enabled for element in favorites)

    magik.button("open-snes-games", Button.A)
    wait_for_label(magik, "Arcade games")
    magik.button("return-to-system-hub", Button.B)
    assert selected_element_with_label(magik, "GAMES").accessible_item_selected

    magik.button("return-to-home", Button.B)
    launcher = element_with_label(magik, "MiSTer MagiK Launcher")
    assert launcher.accessible_enabled
