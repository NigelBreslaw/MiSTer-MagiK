"""In-memory conversion of authoritative RGB565 captures to PNG."""

from __future__ import annotations

import struct
import zlib
from collections.abc import Mapping


def capture_png(
    fields: Mapping[str, object], pixels: bytes, view: str
) -> tuple[bytes, dict]:
    if view not in {"raw", "display"}:
        raise ValueError("view must be raw or display")
    if (
        fields.get("source") != "fpga-latched-scanout-slots"
        or fields.get("pixel_format") != "rgb565-le"
    ):
        raise ValueError("capture is not authoritative RGB565 scanout")
    width, height, stride, sequence = (
        fields.get(key) for key in ("width", "height", "stride_bytes", "frame_sequence")
    )
    if not all(type(value) is int for value in (width, height, stride, sequence)):
        raise ValueError("capture geometry and sequence must be integers")
    if not (
        0 < width <= 1366
        and 0 < height <= 768
        and width * 2 <= stride <= 2736
        and stride % 2 == 0
        and 0 <= sequence <= 65535
    ):
        raise ValueError("invalid capture geometry or sequence")
    if len(pixels) != stride * height:
        raise ValueError("capture payload length does not match geometry")
    rows = []
    for y in range(height):
        row = bytearray()
        for (pixel,) in struct.iter_unpack(
            "<H", pixels[y * stride : y * stride + width * 2]
        ):
            red, green, blue = pixel >> 11, (pixel >> 5) & 63, pixel & 31
            row.extend(
                (
                    (red << 3) | (red >> 2),
                    (green << 2) | (green >> 4),
                    (blue << 3) | (blue >> 2),
                )
            )
        rows.append(bytes(row))
    display = view == "display" and width == 640 and height in {240, 288}
    output_height = 480 if display else height
    raster = b"".join(
        b"\0" + rows[((y * 2 + 1) * height // (output_height * 2)) if display else y]
        for y in range(output_height)
    )
    png = b"\x89PNG\r\n\x1a\n" + _chunk(
        b"IHDR", struct.pack("!IIBBBBB", width, output_height, 8, 2, 0, 0, 0)
    )
    png += _chunk(b"IDAT", zlib.compress(raster)) + _chunk(b"IEND", b"")
    return png, {
        "source": fields["source"],
        "pixel_format": fields["pixel_format"],
        "width": width,
        "height": height,
        "stride_bytes": stride,
        "frame_sequence": sequence,
        "view": view,
        "output_width": width,
        "output_height": output_height,
        "transform": "derived-nearest-scanline-4:3" if display else "none",
    }


def _chunk(kind: bytes, payload: bytes) -> bytes:
    return (
        struct.pack("!I", len(payload))
        + kind
        + payload
        + struct.pack("!I", zlib.crc32(kind + payload))
    )
