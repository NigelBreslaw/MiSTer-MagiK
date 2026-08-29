"""Startup and home navigation journeys."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label


def test_startup_exposes_launcher_and_moves_home_selection(magik: MagiKDriver) -> None:
    launcher = element_with_label(magik, "MiSTer MagiK Launcher")
    assert launcher.accessible_enabled

    before = tuple(
        element.accessible_label
        for element in launcher.query_descendants().find_all()
        if element.accessible_item_selected
    )
    magik.hat("move-home-selection", 1, 0)
    after = tuple(
        element.accessible_label
        for element in launcher.query_descendants().find_all()
        if element.accessible_item_selected
    )
    assert before != after

    # The test deliberately uses a physical face-button event for activation;
    # it never invokes a Slint callback or a Rust action directly.
    magik.button("open-selected-home-item", Button.A)
