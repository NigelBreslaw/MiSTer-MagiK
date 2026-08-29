"""Arcade navigation and keyboard-search journeys."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label, open_arcade, wait_for_label


def test_arcade_list_and_search_keyboard_are_accessible(magik: MagiKDriver) -> None:
    open_arcade(magik)
    games = element_with_label(magik, "Arcade games")
    assert games.accessible_enabled

    magik.button("open-arcade-search", Button.Y)
    keyboard = wait_for_label(magik, "Search keyboard")
    query = wait_for_label(magik, "Search query")
    assert keyboard.accessible_enabled
    assert query.accessible_value == ""

    magik.button("select-search-key", Button.A)
    assert element_with_label(magik, "Search query").accessible_value
