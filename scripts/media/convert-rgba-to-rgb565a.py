#!/usr/bin/env python3
"""Convert an RGBA PNG into MiSTer MagiK's external RGB565A1 asset format."""

from __future__ import annotations

import argparse
import pathlib
import struct
import subprocess
import zlib


MAGIC = b"MM565A1\0"


def dimensions(path: pathlib.Path) -> tuple[int, int]:
    result = subprocess.run(
        ["magick", "identify", "-format", "%w %h", str(path)],
        check=True,
        capture_output=True,
        text=True,
    )
    width, height = (int(value) for value in result.stdout.split())
    if not (1 <= width <= 65535 and 1 <= height <= 65535):
        raise ValueError(f"unsupported image dimensions: {width}x{height}")
    return width, height


def rgba_pixels(path: pathlib.Path, expected_size: int) -> bytes:
    result = subprocess.run(
        ["magick", str(path), "-alpha", "on", "-depth", "8", "rgba:-"],
        check=True,
        capture_output=True,
    )
    if len(result.stdout) != expected_size:
        raise ValueError(
            f"RGBA decoder returned {len(result.stdout)} bytes, expected {expected_size}"
        )
    return result.stdout


def encode(source: pathlib.Path) -> bytes:
    width, height = dimensions(source)
    rgba = rgba_pixels(source, width * height * 4)
    colours = bytearray(width * height * 2)
    alpha = bytearray(width * height)
    for index in range(width * height):
        red, green, blue, opacity = rgba[index * 4 : index * 4 + 4]
        rgb565 = ((red >> 3) << 11) | ((green >> 2) << 5) | (blue >> 3)
        struct.pack_into("<H", colours, index * 2, rgb565)
        alpha[index] = opacity
    payload = bytes(colours + alpha)
    header = struct.pack(
        "<8sHHIII",
        MAGIC,
        width,
        height,
        width * 2,
        width,
        zlib.crc32(payload),
    )
    return header + payload


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    args = parser.parse_args()
    encoded = encode(args.source)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(encoded)


if __name__ == "__main__":
    main()
