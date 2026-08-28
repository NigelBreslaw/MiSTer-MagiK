#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Convert a non-interlaced RGBA PNG into MiSTer MagiK's RLE RGBA sidecar."""

from __future__ import annotations

import struct
import sys
import zlib
from pathlib import Path


def paeth(a: int, b: int, c: int) -> int:
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def read_png_rgba(path: Path) -> tuple[int, int, bytes]:
    data = path.read_bytes()
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError(f"{path} is not a PNG")

    offset = 8
    width = height = None
    idat = bytearray()
    while offset + 8 <= len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        tag = data[offset + 4 : offset + 8]
        payload = data[offset + 8 : offset + 8 + length]
        offset += 12 + length
        if tag == b"IHDR":
            width, height, bit_depth, color_type, compression, filt, interlace = (
                struct.unpack(">IIBBBBB", payload[:13])
            )
            if (bit_depth, color_type, compression, filt, interlace) != (8, 6, 0, 0, 0):
                raise ValueError(
                    f"{path} must be non-interlaced 8-bit RGBA PNG; got "
                    f"bit_depth={bit_depth} color_type={color_type} interlace={interlace}"
                )
        elif tag == b"IDAT":
            idat.extend(payload)
        elif tag == b"IEND":
            break

    if width is None or height is None:
        raise ValueError(f"{path} has no IHDR")

    raw = zlib.decompress(bytes(idat))
    stride = width * 4
    expected = height * (stride + 1)
    if len(raw) != expected:
        raise ValueError(f"{path} decoded to {len(raw)} bytes, expected {expected}")

    rows: list[bytearray] = []
    pos = 0
    for _ in range(height):
        filter_type = raw[pos]
        pos += 1
        row = bytearray(raw[pos : pos + stride])
        pos += stride
        prev = rows[-1] if rows else bytearray(stride)
        for i in range(stride):
            left = row[i - 4] if i >= 4 else 0
            up = prev[i]
            up_left = prev[i - 4] if i >= 4 else 0
            if filter_type == 0:
                pass
            elif filter_type == 1:
                row[i] = (row[i] + left) & 0xFF
            elif filter_type == 2:
                row[i] = (row[i] + up) & 0xFF
            elif filter_type == 3:
                row[i] = (row[i] + ((left + up) // 2)) & 0xFF
            elif filter_type == 4:
                row[i] = (row[i] + paeth(left, up, up_left)) & 0xFF
            else:
                raise ValueError(f"{path} uses unsupported PNG filter {filter_type}")
        rows.append(row)

    return width, height, b"".join(rows)


def main() -> int:
    if len(sys.argv) != 3:
        print(
            "usage: scripts/media/png-to-slint-rgba.py INPUT.png OUTPUT.rgba",
            file=sys.stderr,
        )
        return 2

    src = Path(sys.argv[1])
    dst = Path(sys.argv[2])
    width, height, rgba = read_png_rgba(src)
    chunks = bytearray()
    pos = 0
    while pos < len(rgba):
        pixel = rgba[pos : pos + 4]
        count = 1
        next_pos = pos + 4
        while (
            next_pos < len(rgba)
            and count < 65535
            and rgba[next_pos : next_pos + 4] == pixel
        ):
            count += 1
            next_pos += 4
        chunks.extend(struct.pack("<H", count))
        chunks.extend(pixel)
        pos = next_pos

    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_bytes(
        f"MISTER_MAGIK_RGBA_RLE\n{width} {height}\n".encode("ascii") + bytes(chunks)
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
