"""Bounded host decoder for probe-produced RGB565 preview frames."""

from __future__ import annotations

import struct
from dataclasses import dataclass


MAGIC = b"MMFSv1\0\0"
HEADER_LEN = 72
MAX_SURFACE_BYTES = 16 * 1024 * 1024


class FrameError(RuntimeError):
    """The device preview does not conform to the shared frame wire format."""


@dataclass(frozen=True)
class PreviewFrame:
    sequence: int
    width: int
    height: int
    stride_pixels: int
    pixels: bytes


def decode_preview(raw: bytes) -> PreviewFrame:
    if len(raw) < HEADER_LEN or raw[:8] != MAGIC:
        raise FrameError("invalid preview frame header")
    kind = raw[8]
    header_len = struct.unpack_from("<H", raw, 12)[0]
    if kind != 2 or header_len != HEADER_LEN:
        raise FrameError("preview is not a complete keyframe")
    sequence = struct.unpack_from("<Q", raw, 16)[0]
    width, height, stride = struct.unpack_from("<III", raw, 32)
    x, y, rect_width, rect_height, raw_bytes, payload_bytes = struct.unpack_from("<IIIIII", raw, 44)
    if width == 0 or height == 0 or stride < width or (x, y, rect_width, rect_height) != (0, 0, width, height):
        raise FrameError("invalid preview geometry")
    expected = width * height * 2
    if expected > MAX_SURFACE_BYTES or raw_bytes != expected or payload_bytes != expected:
        raise FrameError("invalid preview payload length")
    if len(raw) != HEADER_LEN + payload_bytes:
        raise FrameError("truncated preview payload")
    return PreviewFrame(sequence, width, height, stride, raw[HEADER_LEN:])
