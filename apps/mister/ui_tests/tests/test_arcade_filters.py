"""Arcade filters, favourites confirmation, and return navigation."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import (
    element_with_label,
    open_arcade,
    selected_labels,
)


def test_arcade_filter_drawer_and_favourite_confirmation(magik: MagiKDriver) -> None:
    open_arcade(magik)
    magik.hat("open-arcade-filters", -1, 0)
    drawer = element_with_label(magik, "Games A-Z")
    assert drawer.accessible_enabled
    assert drawer.accessible_description == "Games A-Z"

    magik.hat("choose-next-letter", 0, 1)
    selected = selected_labels(magik)
    assert selected
    magik.button("apply-letter-filter", Button.A)
    assert element_with_label(magik, "Arcade games").accessible_enabled

    magik.button("open-favourite-confirmation", Button.X)
    confirmation = element_with_label(magik, "Confirmation")
    assert confirmation.accessible_description
    magik.button("cancel-favourite-confirmation", Button.B)


def test_arcade_back_returns_to_launcher(magik: MagiKDriver) -> None:
    open_arcade(magik)
    magik.button("leave-arcade", Button.B)
    launcher = element_with_label(magik, "MiSTer MagiK Launcher")
    assert launcher.accessible_enabled
    assert "Arcade" in selected_labels(magik)
