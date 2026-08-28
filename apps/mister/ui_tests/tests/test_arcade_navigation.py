"""Arcade navigation and keyboard-search journeys."""

from __future__ import annotations

from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label, selected_labels
from apps.mister.ui_tests.uinput_joystick import Button


def _open_arcade(driver: MagiKDriver) -> None:
    for step in range(16):
        if "Arcade" in selected_labels(driver):
            break
        driver.hat(f"find-arcade-{step}", 1, 0)
    else:
        raise AssertionError("Arcade home item never became selected")
    driver.button("open-arcade-hub", Button.A)
    for step in range(4):
        if "Games" in selected_labels(driver):
            break
        driver.hat(f"find-games-{step}", 0, 1)
    driver.button("open-arcade-games", Button.A)


def test_arcade_list_and_search_keyboard_are_accessible(magik: MagiKDriver) -> None:
    _open_arcade(magik)
    games = element_with_label(magik, "Arcade games")
    assert games.accessible_enabled

    magik.button("open-arcade-search", Button.Y)
    keyboard = element_with_label(magik, "Search keyboard")
    query = element_with_label(magik, "Search query")
    assert keyboard.accessible_enabled
    assert query.accessible_value == ""

    magik.button("select-search-key", Button.A)
    assert element_with_label(magik, "Search query").accessible_value
