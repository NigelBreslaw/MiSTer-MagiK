from __future__ import annotations

import struct

import pytest

from magik2.frames import FrameError, decode_preview


def keyframe(payload: bytes = b"\0" * 8) -> bytes:
    header = bytearray(72)
    header[:8] = b"MMFSv1\0\0"
    header[8] = 2
    struct.pack_into("<H", header, 12, 72)
    struct.pack_into("<Q", header, 16, 4)
    struct.pack_into("<III", header, 32, 2, 2, 2)
    struct.pack_into("<IIIIII", header, 44, 0, 0, 2, 2, len(payload), len(payload))
    return bytes(header) + payload


def test_decodes_shared_rgb565_keyframe() -> None:
    frame = decode_preview(keyframe())
    assert (frame.sequence, frame.width, frame.height, frame.pixels) == (
        4,
        2,
        2,
        b"\0" * 8,
    )


def test_rejects_truncated_or_non_keyframe_preview() -> None:
    with pytest.raises(FrameError, match="truncated"):
        decode_preview(keyframe()[:-1])
    malformed = bytearray(keyframe())
    malformed[8] = 3
    with pytest.raises(FrameError, match="keyframe"):
        decode_preview(bytes(malformed))
