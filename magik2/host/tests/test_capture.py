from __future__ import annotations

import base64
import json
import os
import struct
import sys
import zlib
from pathlib import Path

import anyio
import pytest
from magik2.capture import capture_png
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


def fields(width=3, height=1, stride_bytes=8):
    return {
        "source": "fpga-latched-scanout-slots",
        "pixel_format": "rgb565-le",
        "width": width,
        "height": height,
        "stride_bytes": stride_bytes,
        "frame_sequence": 7,
    }


def decode_png(png):
    assert png[:8] == b"\x89PNG\r\n\x1a\n"
    offset, compressed, geometry = 8, b"", None
    while offset < len(png):
        length = struct.unpack_from("!I", png, offset)[0]
        kind = png[offset + 4 : offset + 8]
        payload = png[offset + 8 : offset + 8 + length]
        assert (
            zlib.crc32(kind + payload)
            == struct.unpack_from("!I", png, offset + 8 + length)[0]
        )
        if kind == b"IHDR":
            geometry = struct.unpack("!IIBBBBB", payload)
        if kind == b"IDAT":
            compressed += payload
        offset += length + 12
    return geometry, zlib.decompress(compressed)


def test_rgb565_conversion_ignores_stride_padding():
    png, metadata = capture_png(
        fields(), struct.pack("<4H", 0xF800, 0x07E0, 0x001F, 0xFFFF), "raw"
    )
    geometry, pixels = decode_png(png)
    assert geometry == (3, 1, 8, 2, 0, 0, 0)
    assert pixels == b"\0\xff\0\0\0\xff\0\0\0\xff"
    assert metadata["transform"] == "none"
    assert metadata["frame_sequence"] == 7


@pytest.mark.parametrize("height", [240, 288])
def test_display_view_samples_centred_scanlines_without_blending(height):
    source = b"".join(struct.pack("<H", y % 32) * 640 for y in range(height))
    png, metadata = capture_png(fields(640, height, 1280), source, "display")
    geometry, pixels = decode_png(png)
    assert geometry[:2] == (640, 480)
    for y in range(480):
        blue = ((y * 2 + 1) * height // 960) % 32
        assert pixels[y * 1921 + 1 : y * 1921 + 4] == bytes(
            (0, 0, (blue << 3) | (blue >> 2))
        )
    assert metadata["height"] == height
    assert metadata["transform"] == "derived-nearest-scanline-4:3"


def test_display_leaves_other_geometries_unchanged():
    assert (
        capture_png(fields(), bytes(8), "display")[0]
        == capture_png(fields(), bytes(8), "raw")[0]
    )


@pytest.mark.parametrize(
    "change",
    [
        {"width": 0},
        {"width": 1367},
        {"height": 769},
        {"stride_bytes": 5},
        {"stride_bytes": 2738},
        {"height": True},
        {"frame_sequence": -1},
        {"source": "preview"},
        {"pixel_format": "rgb888"},
    ],
)
def test_invalid_capture_metadata_is_rejected(change):
    with pytest.raises(ValueError):
        capture_png(fields() | change, bytes(8), "raw")


@pytest.mark.parametrize("size", [0, 7, 9])
def test_payload_must_match_geometry_exactly(size):
    with pytest.raises(ValueError, match="payload length"):
        capture_png(fields(), bytes(size), "raw")


def test_mcp_stdio_discovery_image_and_errors(tmp_path):
    # Real SDK/stdio session; only the device connection is replaced. Child
    # process output simulates build/bootstrap chatter and must stay off stdout.
    script = """
import subprocess
from magik2 import mcp_capture
class Agent:
    def capture_framebuffer(self):
        return {"source":"fpga-latched-scanout-slots", "pixel_format":"rgb565-le", "width":1, "height":1, "stride_bytes":2, "frame_sequence":9}, b"\\x00\\xf8"
def connect(*args):
    subprocess.run(["echo", "bootstrap output"], check=True)
    return Agent(), None
mcp_capture.connect_agent = connect
mcp_capture.main()
"""

    async def run():
        params = StdioServerParameters(
            command=sys.executable,
            args=["-c", script],
            env={
                **os.environ,
                "MISTER_IP": "fixture",
                "MISTER_MAGIK2_RESULTS": str(tmp_path),
                "PYTHONPATH": str(Path(__file__).resolve().parents[1]),
            },
        )
        with anyio.fail_after(10):
            async with stdio_client(params) as (reader, writer):
                async with ClientSession(reader, writer) as session:
                    await session.initialize()
                    assert [
                        tool.name for tool in (await session.list_tools()).tools
                    ] == ["capture_framebuffer"]
                    result = await session.call_tool("capture_framebuffer", {})
                    assert not result.isError
                    image, metadata = result.content
                    assert image.type == "image" and image.mimeType == "image/png"
                    assert (
                        decode_png(base64.b64decode(image.data, validate=True))[1]
                        == b"\0\xff\0\0"
                    )
                    assert json.loads(metadata.text)["frame_sequence"] == 9
                    error = await session.call_tool(
                        "capture_framebuffer", {"view": "invalid"}
                    )
                    assert error.isError and all(
                        part.type == "text" for part in error.content
                    )

    anyio.run(run)
    assert not list(tmp_path.rglob("*.png"))
    assert all("data" not in p.read_text() for p in tmp_path.rglob("events.jsonl"))


def test_mcp_capture_error_is_text_without_an_image(monkeypatch):
    from magik2 import mcp_capture

    def fail(view):
        raise TimeoutError("capture deadline exceeded")

    monkeypatch.setattr(mcp_capture, "capture_image", fail)
    result = anyio.run(mcp_capture.call_tool, "capture_framebuffer", {})
    assert result.isError
    assert [part.type for part in result.content] == ["text"]
    assert "deadline" in result.content[0].text
