"""Correlate logical agent input with observable Slint state."""

from __future__ import annotations

import time
from collections.abc import Callable
from dataclasses import dataclass

from .agent_input import AgentInput, Button, Key
from .slint_adapter import SlintElement


def _element_snapshot(
    element: SlintElement,
) -> tuple[str, str, str, bool, bool, bool, bool]:
    properties = element._get_props()
    return (
        properties.accessible_label,
        properties.accessible_description,
        properties.accessible_value,
        properties.accessible_checked,
        properties.accessible_enabled,
        properties.accessible_item_selected,
        bool(properties.accessible_label),
    )


@dataclass(frozen=True)
class AccessibilitySnapshot:
    """Stable, serializable projection of the accessibility tree."""

    elements: tuple[tuple[str, str, str, bool, bool, bool, bool], ...]

    @classmethod
    def capture(
        cls,
        root: SlintElement,
        keep_alive: Callable[[], None] | None = None,
    ) -> AccessibilitySnapshot:
        if keep_alive is not None:
            keep_alive()
        next_keep_alive = time.monotonic() + 2.0
        values = []
        for element in (root, *root.query_descendants().find_all()):
            values.append(_element_snapshot(element))
            if keep_alive is not None and time.monotonic() >= next_keep_alive:
                keep_alive()
                next_keep_alive = time.monotonic() + 2.0
        return cls(tuple(sorted(values)))

    def labels(self) -> tuple[str, ...]:
        return tuple(element[0] for element in self.elements if element[6])

    def selected_labels(self) -> tuple[str, ...]:
        return tuple(element[0] for element in self.elements if element[5])


@dataclass(frozen=True)
class CorrelatedInput:
    """One logical action and the state transition it caused."""

    action: str
    source: str
    before: AccessibilitySnapshot
    after: AccessibilitySnapshot


class InputCorrelation:
    """Send logical agent input and await an accessibility-observable transition."""

    def __init__(
        self,
        root: SlintElement,
        inputs: AgentInput,
    ) -> None:
        self._root = root
        self._inputs = inputs
        self._history: list[CorrelatedInput] = []

    def key(self, action: str, key: Key, timeout: float = 2.0) -> CorrelatedInput:
        return self._record(action, "keyboard", lambda: self._inputs.key(key), timeout)

    def button(
        self, action: str, button: Button, timeout: float = 2.0
    ) -> CorrelatedInput:
        return self._record(
            action, "joystick", lambda: self._inputs.button(button), timeout
        )

    def hat(
        self,
        action: str,
        horizontal: int,
        vertical: int,
        timeout: float = 2.0,
    ) -> CorrelatedInput:
        result = self._record(
            action,
            "joystick",
            lambda: self._inputs.hat(horizontal, vertical),
            timeout,
        )
        self._inputs.hat(0, 0)
        return result

    def history(self) -> tuple[CorrelatedInput, ...]:
        return tuple(self._history)

    def keep_alive(self) -> None:
        self._inputs.keep_alive()

    def _record(
        self,
        action: str,
        source: str,
        send: Callable[[], None],
        timeout: float,
    ) -> CorrelatedInput:
        before = AccessibilitySnapshot.capture(self._root, self._inputs.keep_alive)
        send()
        deadline = time.monotonic() + timeout
        after = before
        while time.monotonic() < deadline:
            after = AccessibilitySnapshot.capture(self._root, self._inputs.keep_alive)
            if after != before:
                result = CorrelatedInput(action, source, before, after)
                self._history.append(result)
                return result
            time.sleep(0.02)
        raise AssertionError(
            f"{source} action {action!r} produced no accessibility transition"
        )


__all__ = ["AccessibilitySnapshot", "CorrelatedInput", "InputCorrelation"]
