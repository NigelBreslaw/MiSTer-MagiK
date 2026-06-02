#!/usr/bin/env python3
"""Convert a raw MiSTer framebuffer dump (/dev/fb0) into a PNG.

The MiSTer framebuffer is 32-bit little-endian with the channel layout
red@16, green@8, blue@0 (i.e. bytes are B, G, R, X), no row padding. This
lets us "see" what is on the MiSTer's HDMI output from a machine that has no
access to the screen.

Usage: raw_to_png.py <raw> <width> <height> <out.png>
"""
import struct
import sys
import zlib


def raw_bgrx_to_png(raw: bytes, w: int, h: int, out_path: str) -> None:
    rowstride = w * 4
    buf = bytearray()
    for y in range(h):
        buf.append(0)  # PNG "none" filter for this scanline
        row = bytearray(raw[y * rowstride : (y + 1) * rowstride])
        # Swap B<->R (RHS is fully evaluated before assignment, so this is safe).
        row[0::4], row[2::4] = bytes(row[2::4]), bytes(row[0::4])
        row[3::4] = b"\xff" * w  # force opaque alpha
        buf += row

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(buf), 6))
    png += chunk(b"IEND", b"")
    with open(out_path, "wb") as f:
        f.write(png)


def main() -> int:
    if len(sys.argv) != 5:
        print(__doc__)
        return 2
    raw_path, w, h, out_path = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
    with open(raw_path, "rb") as f:
        raw = f.read()
    needed = w * h * 4
    if len(raw) < needed:
        print(f"raw is {len(raw)} bytes, need {needed} for {w}x{h}x32", file=sys.stderr)
        return 1
    raw_bgrx_to_png(raw[:needed], w, h, out_path)
    print(f"wrote {out_path} ({w}x{h})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
