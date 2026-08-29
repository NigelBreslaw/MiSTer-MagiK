"""Keyboard-only input through Linux's uinput device."""

from __future__ import annotations

import fcntl
import struct
import time
from collections.abc import Iterable
from enum import IntEnum
from types import TracebackType
from typing import BinaryIO, Self

_UI_SET_EVBIT = 0x40045564
_UI_SET_KEYBIT = 0x40045565
_UI_DEV_CREATE = 0x5501
_UI_DEV_DESTROY = 0x5502
_EV_SYN = 0
_EV_KEY = 1
_BUS_USB = 0x03
_UINPUT_MAX_NAME_SIZE = 80
_ABS_CNT = 64
_EVENT = struct.Struct("llHHI")


class Key(IntEnum):
    """Linux key codes used by the MagiK launcher."""

    ESCAPE = 1
    ENTER = 28
    SPACE = 57
    TAB = 15
    BACKSPACE = 14
    HOME = 102
    UP = 103
    LEFT = 105
    RIGHT = 106
    DOWN = 108
    PAGE_UP = 104
    PAGE_DOWN = 109
    F9 = 67
    F10 = 68
    F12 = 88
    MENU = 139


def _user_dev(name: str) -> bytes:
    """Encode the legacy ``uinput_user_dev`` structure for this ABI."""

    data = bytearray(struct.calcsize("80sHHHHi") + struct.calcsize(f"{_ABS_CNT * 4}i"))
    struct.pack_into(
        "80sHHHHi",
        data,
        0,
        name.encode("ascii", errors="replace")[: _UINPUT_MAX_NAME_SIZE - 1],
        _BUS_USB,
        0x1209,
        0x0001,
        1,
        0,
    )
    return bytes(data)


class VirtualKeyboard:
    """A scoped virtual keyboard that emits real Linux input events."""

    def __init__(
        self,
        device: str = "/dev/uinput",
        keys: Iterable[Key] = tuple(Key),
    ) -> None:
        self._device_path = device
        self._keys = tuple(keys)
        self._file: BinaryIO | None = None
        self._held: set[Key] = set()

    def __enter__(self) -> Self:
        try:
            file = open(self._device_path, "wb", buffering=0)
            self._file = file
            fcntl.ioctl(file, _UI_SET_EVBIT, _EV_KEY)
            fcntl.ioctl(file, _UI_SET_EVBIT, _EV_SYN)
            for key in self._keys:
                fcntl.ioctl(file, _UI_SET_KEYBIT, int(key))
            file.write(_user_dev("MiSTer MagiK UI test keyboard"))
            fcntl.ioctl(file, _UI_DEV_CREATE)
            time.sleep(0.05)
            return self
        except OSError as error:
            self.close()
            raise RuntimeError(
                f"unable to create {self._device_path}; UI tests need uinput access"
            ) from error

    def __exit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()

    def press(self, key: Key) -> None:
        if key not in self._keys:
            raise ValueError(f"key {key.name} was not registered")
        if key in self._held:
            return
        self._emit(key, 1)
        self._held.add(key)

    def release(self, key: Key) -> None:
        if key not in self._held:
            return
        self._emit(key, 0)
        self._held.remove(key)

    def tap(self, key: Key, hold_seconds: float = 0.03) -> None:
        self.press(key)
        time.sleep(hold_seconds)
        self.release(key)

    def close(self) -> None:
        file = self._file
        if file is None:
            return
        for key in tuple(self._held):
            self.release(key)
        try:
            fcntl.ioctl(file, _UI_DEV_DESTROY)
        except OSError:
            pass
        file.close()
        self._file = None

    def _emit(self, key: Key, value: int) -> None:
        file = self._file
        if file is None:
            raise RuntimeError("virtual keyboard is not active")
        now = time.time_ns()
        file.write(
            _EVENT.pack(
                now // 1_000_000_000,
                (now % 1_000_000_000) // 1_000,
                _EV_KEY,
                int(key),
                value,
            )
        )
        file.write(_EVENT.pack(0, 0, _EV_SYN, 0, 0))


__all__ = ["Key", "VirtualKeyboard"]
