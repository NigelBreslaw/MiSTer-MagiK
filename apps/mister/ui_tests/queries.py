"""Small accessibility-tree queries shared by device UI tests."""

from __future__ import annotations

import time

from .agent_input import Button
from .driver import MagiKDriver
from .slint_adapter import SlintElement, SlintElementProperties


def _elements_including_root(driver: MagiKDriver) -> tuple[SlintElement, ...]:
    root = driver.window.root_element
    return (root, *root.query_descendants().find_all())


def _elements_with_properties(
    driver: MagiKDriver,
) -> tuple[tuple[SlintElement, SlintElementProperties], ...]:
    driver.keep_alive()
    next_keep_alive = time.monotonic() + 2.0
    values = []
    for element in _elements_including_root(driver):
        values.append((element, element._get_props()))
        if time.monotonic() >= next_keep_alive:
            driver.keep_alive()
            next_keep_alive = time.monotonic() + 2.0
    return tuple(values)


def element_with_label(driver: MagiKDriver, label: str) -> SlintElement:
    matches = elements_with_label(driver, label)
    if len(matches) != 1:
        raise AssertionError(
            f"expected one element labeled {label!r}, got {len(matches)}"
        )
    return matches[0]


def elements_with_label(driver: MagiKDriver, label: str) -> tuple[SlintElement, ...]:
    return tuple(
        element
        for element, properties in _elements_with_properties(driver)
        if properties.accessible_label == label
    )


def element_with_any_label(
    driver: MagiKDriver, labels: tuple[str, ...]
) -> SlintElement:
    counts: dict[str, int] = {}
    for label in labels:
        matches = [
            element
            for element, properties in _elements_with_properties(driver)
            if properties.accessible_label == label
        ]
        counts[label] = len(matches)
        if len(matches) == 1:
            return matches[0]
    raise AssertionError(f"none of the labels were unique: {counts!r}")


def selected_labels(driver: MagiKDriver) -> tuple[str, ...]:
    return tuple(
        properties.accessible_label
        for _element, properties in _elements_with_properties(driver)
        if properties.accessible_item_selected and properties.accessible_label
    )


def selected_element_with_label(driver: MagiKDriver, label: str) -> SlintElement:
    matches = [
        element
        for element, properties in _elements_with_properties(driver)
        if properties.accessible_label == label and properties.accessible_item_selected
    ]
    if len(matches) != 1:
        raise AssertionError(
            f"expected one selected element labeled {label!r}, got {len(matches)}"
        )
    return matches[0]


def wait_for_label(
    driver: MagiKDriver, label: str, timeout: float = 2.0
) -> SlintElement:
    deadline = time.monotonic() + timeout
    while True:
        matches = [
            element
            for element, properties in _elements_with_properties(driver)
            if properties.accessible_label == label
        ]
        if matches:
            return matches[0]
        if time.monotonic() >= deadline:
            raise AssertionError(f"label {label!r} did not appear within {timeout}s")
        time.sleep(0.02)


def wait_for_unique_label(
    driver: MagiKDriver, label: str, timeout: float = 2.0
) -> SlintElement:
    deadline = time.monotonic() + timeout
    count = 0
    while True:
        matches = [
            element
            for element, properties in _elements_with_properties(driver)
            if properties.accessible_label == label
        ]
        count = len(matches)
        if count == 1:
            return matches[0]
        if time.monotonic() >= deadline:
            raise AssertionError(
                f"label {label!r} did not become unique within {timeout}s; got {count}"
            )
        time.sleep(0.02)


def open_arcade(driver: MagiKDriver) -> None:
    for step in range(16):
        if "Arcade" in selected_labels(driver):
            break
        driver.hat(f"find-arcade-{step}", 1, 0)
    else:
        raise AssertionError("Arcade home item never became selected")
    driver.button("open-arcade-games", Button.A)
    wait_for_label(driver, "Arcade games")


def open_settings(driver: MagiKDriver) -> None:
    driver.hat("focus-settings", 0, -1)
    driver.button("open-settings", Button.A)
    wait_for_label(driver, "Settings")


__all__ = [
    "element_with_any_label",
    "element_with_label",
    "elements_with_label",
    "open_arcade",
    "open_settings",
    "selected_element_with_label",
    "selected_labels",
    "wait_for_label",
    "wait_for_unique_label",
]
