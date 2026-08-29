"""Gamepad input through Linux's uinput device."""

from __future__ import annotations

import fcntl
import struct
import time
from enum import IntEnum
from types import TracebackType
from typing import BinaryIO, Self

_UI_SET_EVBIT = 0x40045564
_UI_SET_KEYBIT = 0x40045565
_UI_SET_ABSBIT = 0x40045567
_UI_DEV_CREATE = 0x5501
_UI_DEV_DESTROY = 0x5502
_EV_SYN = 0
_EV_KEY = 1
_EV_ABS = 3
_BUS_USB = 0x03
_ABS_CNT = 64
_ABS_X = 0
_ABS_Y = 1
_ABS_RX = 3
_ABS_RY = 4
_ABS_HAT0X = 16
_ABS_HAT0Y = 17
_AXIS_MAX = 32767
_EVENT = struct.Struct("llHHI")


class Button(IntEnum):
    """SNES-style buttons understood by the MagiK input mapper."""

    A = 304
    B = 305
    X = 307
    Y = 308
    L = 310
    R = 311
    ZL = 312
    ZR = 313
    SELECT = 314
    START = 315
    HOME = 316
    CAPTURE = 318


def _user_dev(name: str) -> bytearray:
    """Encode ``uinput_user_dev`` with explicit ranges for our axes."""

    header = struct.calcsize("80sHHHHi")
    data = bytearray(header + struct.calcsize(f"{_ABS_CNT * 4}i"))
    struct.pack_into(
        "80sHHHHi",
        data,
        0,
        name.encode("ascii", errors="replace")[:79],
        _BUS_USB,
        0x1209,
        0x0002,
        1,
        0,
    )
    # The four arrays follow absmax, absmin, absfuzz, absflat order.
    array_bytes = _ABS_CNT * struct.calcsize("i")
    for axis in (_ABS_X, _ABS_Y, _ABS_RX, _ABS_RY):
        struct.pack_into("i", data, header + axis * struct.calcsize("i"), _AXIS_MAX)
        struct.pack_into(
            "i", data, header + array_bytes + axis * struct.calcsize("i"), -_AXIS_MAX
        )
    for axis in (_ABS_HAT0X, _ABS_HAT0Y):
        struct.pack_into("i", data, header + axis * struct.calcsize("i"), 1)
        struct.pack_into(
            "i", data, header + array_bytes + axis * struct.calcsize("i"), -1
        )
    return data


class VirtualJoystick:
    """A scoped virtual joystick emitting real Linux input events."""

    def __init__(self, device: str = "/dev/uinput") -> None:
        self._device_path = device
        self._file: BinaryIO | None = None
        self._buttons: set[Button] = set()
        self._hat = (0, 0)

    def __enter__(self) -> Self:
        try:
            file = open(self._device_path, "wb", buffering=0)
            self._file = file
            fcntl.ioctl(file, _UI_SET_EVBIT, _EV_KEY)
            fcntl.ioctl(file, _UI_SET_EVBIT, _EV_ABS)
            fcntl.ioctl(file, _UI_SET_EVBIT, _EV_SYN)
            for button in Button:
                fcntl.ioctl(file, _UI_SET_KEYBIT, int(button))
            for axis in (_ABS_X, _ABS_Y, _ABS_RX, _ABS_RY, _ABS_HAT0X, _ABS_HAT0Y):
                fcntl.ioctl(file, _UI_SET_ABSBIT, axis)
            file.write(_user_dev("MiSTer MagiK UI test joystick"))
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

    def press(self, button: Button) -> None:
        if button in self._buttons:
            return
        self._emit(_EV_KEY, button, 1)
        self._buttons.add(button)

    def release(self, button: Button) -> None:
        if button not in self._buttons:
            return
        self._emit(_EV_KEY, button, 0)
        self._buttons.remove(button)

    def tap(self, button: Button, hold_seconds: float = 0.03) -> None:
        self.press(button)
        time.sleep(hold_seconds)
        self.release(button)

    def hat(self, horizontal: int, vertical: int) -> None:
        """Set the D-pad to -1, 0, or 1 on each axis."""

        requested = (max(-1, min(1, horizontal)), max(-1, min(1, vertical)))
        if requested == self._hat:
            return
        self._emit(_EV_ABS, _ABS_HAT0X, requested[0])
        self._emit(_EV_ABS, _ABS_HAT0Y, requested[1])
        self._hat = requested

    def axes(self, horizontal: int, vertical: int) -> None:
        """Set both analog axes in the Linux -32767..32767 range."""

        self._emit(_EV_ABS, _ABS_X, max(-_AXIS_MAX, min(_AXIS_MAX, horizontal)))
        self._emit(_EV_ABS, _ABS_Y, max(-_AXIS_MAX, min(_AXIS_MAX, vertical)))

    def neutral(self) -> None:
        self.hat(0, 0)
        self.axes(0, 0)

    def close(self) -> None:
        file = self._file
        if file is None:
            return
        for button in tuple(self._buttons):
            self.release(button)
        self.neutral()
        try:
            fcntl.ioctl(file, _UI_DEV_DESTROY)
        except OSError:
            pass
        file.close()
        self._file = None

    def _emit(self, event_type: int, code: int | Button, value: int) -> None:
        file = self._file
        if file is None:
            raise RuntimeError("virtual joystick is not active")
        now = time.time_ns()
        file.write(
            _EVENT.pack(
                now // 1_000_000_000,
                (now % 1_000_000_000) // 1_000,
                event_type,
                int(code),
                value,
            )
        )
        file.write(_EVENT.pack(0, 0, _EV_SYN, 0, 0))


__all__ = ["Button", "VirtualJoystick"]
