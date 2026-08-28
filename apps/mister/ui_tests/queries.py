"""Small accessibility-tree queries shared by device UI tests."""

from __future__ import annotations

from .driver import MagiKDriver
from .slint_adapter import SlintElement


def element_with_label(driver: MagiKDriver, label: str) -> SlintElement:
    matches = [
        element
        for element in driver.window.root_element.query_descendants().find_all()
        if element.accessible_label == label
    ]
    if len(matches) != 1:
        raise AssertionError(
            f"expected one element labeled {label!r}, got {len(matches)}"
        )
    return matches[0]


def selected_labels(driver: MagiKDriver) -> tuple[str, ...]:
    return tuple(
        element.accessible_label
        for element in driver.window.root_element.query_descendants().find_all()
        if element.accessible_item_selected and element.accessible_label
    )


__all__ = ["element_with_label", "selected_labels"]
