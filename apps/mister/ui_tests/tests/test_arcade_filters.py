"""Arcade filters, favourites confirmation, and recent/favourites hubs."""

from __future__ import annotations

from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label, open_arcade, selected_labels
from apps.mister.ui_tests.uinput_joystick import Button


def test_arcade_filter_drawer_and_favourite_confirmation(magik: MagiKDriver) -> None:
    open_arcade(magik)
    magik.button("open-arcade-filters", Button.X)
    drawer = element_with_label(magik, "Filters")
    assert drawer.accessible_enabled
    assert drawer.accessible_description == "Games A-Z"

    magik.hat("choose-category-filter", 0, 1)
    selected = selected_labels(magik)
    assert selected
    magik.button("apply-category-filter", Button.A)
    assert element_with_label(magik, "Arcade games").accessible_enabled

    magik.button("open-favourite-confirmation", Button.X)
    confirmation = element_with_label(magik, "Confirmation")
    assert confirmation.accessible_description
    magik.button("cancel-favourite-confirmation", Button.B)


def test_system_hub_exposes_recent_and_favourites_cards(magik: MagiKDriver) -> None:
    open_arcade(magik)
    magik.button("leave-arcade", Button.B)
    element_with_label(magik, "RECENT")
    element_with_label(magik, "FAVORITES")
