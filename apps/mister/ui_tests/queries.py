"""Small accessibility-tree queries shared by device UI tests."""

from __future__ import annotations

from .agent_input import Button
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


def element_with_any_label(
    driver: MagiKDriver, labels: tuple[str, ...]
) -> SlintElement:
    for label in labels:
        matches = [
            element
            for element in driver.window.root_element.query_descendants().find_all()
            if element.accessible_label == label
        ]
        if len(matches) == 1:
            return matches[0]
    raise AssertionError(f"none of the labels were found: {labels!r}")


def selected_labels(driver: MagiKDriver) -> tuple[str, ...]:
    return tuple(
        element.accessible_label
        for element in driver.window.root_element.query_descendants().find_all()
        if element.accessible_item_selected and element.accessible_label
    )


def open_arcade(driver: MagiKDriver) -> None:
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


__all__ = [
    "element_with_any_label",
    "element_with_label",
    "open_arcade",
    "selected_labels",
]
