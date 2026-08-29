"""Display and orientation settings journeys."""

from __future__ import annotations

from apps.mister.ui_tests.agent_input import Button, Key
from apps.mister.ui_tests.driver import MagiKDriver
from apps.mister.ui_tests.queries import element_with_any_label, element_with_label


def test_display_and_orientation_combos_are_keyboard_driven(
    magik: MagiKDriver,
) -> None:
    magik.key("open-settings", Key.F12)
    element_with_label(magik, "Settings")

    magik.button("open-display-resolution", Button.A)
    display = element_with_any_label(magik, ("Display resolution", "Resolution"))
    assert display.accessible_description
    magik.hat("highlight-next-display", 0, 1)
    magik.button("cancel-display-resolution", Button.B)

    magik.hat("select-orientation-setting", 0, 1)
    magik.button("open-screen-orientation", Button.A)
    orientation = element_with_any_label(magik, ("Screen orientation", "Orientation"))
    assert orientation.accessible_description
    magik.button("cancel-screen-orientation", Button.B)
