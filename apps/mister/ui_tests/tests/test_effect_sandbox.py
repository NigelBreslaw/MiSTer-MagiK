"""Smoke journeys for menu effects while keeping device mutations sandboxed."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_label, open_settings


def test_exit_and_rebuild_are_confirmation_only(magik: MagiKDriver) -> None:
    open_settings(magik)
    for step in range(4):
        magik.hat(f"select-exit-{step}", 0, 1)
    magik.button("open-exit-confirmation", Button.A)
    confirmation = element_with_label(magik, "Confirmation")
    assert confirmation.accessible_description
    magik.button("cancel-exit", Button.B)

    magik.hat("select-rebuild", 0, 1)
    magik.button("open-rebuild-confirmation", Button.A)
    element_with_label(magik, "Confirmation")
    magik.button("cancel-rebuild", Button.B)
