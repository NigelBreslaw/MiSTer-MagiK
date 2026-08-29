"""Correlate physical uinput events with observable Slint state."""

from __future__ import annotations

import time
from collections.abc import Callable
from dataclasses import dataclass

from .slint_adapter import SlintElement
from .uinput_joystick import Button, VirtualJoystick
from .uinput_keyboard import Key, VirtualKeyboard


@dataclass(frozen=True)
class AccessibilitySnapshot:
    """Stable, serializable projection of the accessibility tree."""

    elements: tuple[tuple[str, str, str, bool, bool, bool, bool], ...]

    @classmethod
    def capture(cls, root: SlintElement) -> AccessibilitySnapshot:
        values = [
            (
                element.accessible_label,
                element.accessible_description,
                element.accessible_value,
                element.accessible_checked,
                element.accessible_enabled,
                element.accessible_item_selected,
                bool(element.accessible_label),
            )
            for element in root.query_descendants().find_all()
        ]
        return cls(tuple(sorted(values)))

    def labels(self) -> tuple[str, ...]:
        return tuple(element[0] for element in self.elements if element[6])

    def selected_labels(self) -> tuple[str, ...]:
        return tuple(element[0] for element in self.elements if element[5])


@dataclass(frozen=True)
class CorrelatedInput:
    """One physical action and the state transition it caused."""

    action: str
    source: str
    before: AccessibilitySnapshot
    after: AccessibilitySnapshot


class InputCorrelation:
    """Drive uinput and wait for an accessibility-observable transition."""

    def __init__(
        self,
        root: SlintElement,
        keyboard: VirtualKeyboard,
        joystick: VirtualJoystick,
    ) -> None:
        self._root = root
        self._keyboard = keyboard
        self._joystick = joystick
        self._history: list[CorrelatedInput] = []

    def key(self, action: str, key: Key, timeout: float = 2.0) -> CorrelatedInput:
        return self._record(
            action, "keyboard", lambda: self._keyboard.tap(key), timeout
        )

    def button(
        self, action: str, button: Button, timeout: float = 2.0
    ) -> CorrelatedInput:
        return self._record(
            action, "joystick", lambda: self._joystick.tap(button), timeout
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
            lambda: self._joystick.hat(horizontal, vertical),
            timeout,
        )
        self._joystick.hat(0, 0)
        return result

    def history(self) -> tuple[CorrelatedInput, ...]:
        return tuple(self._history)

    def _record(
        self,
        action: str,
        source: str,
        send: Callable[[], None],
        timeout: float,
    ) -> CorrelatedInput:
        before = AccessibilitySnapshot.capture(self._root)
        send()
        deadline = time.monotonic() + timeout
        after = before
        while time.monotonic() < deadline:
            after = AccessibilitySnapshot.capture(self._root)
            if after != before:
                result = CorrelatedInput(action, source, before, after)
                self._history.append(result)
                return result
            time.sleep(0.02)
        raise AssertionError(
            f"{source} action {action!r} produced no accessibility transition"
        )


__all__ = ["AccessibilitySnapshot", "CorrelatedInput", "InputCorrelation"]
