"""A single-purpose stdio MCP server for direct framebuffer image delivery."""

from __future__ import annotations

import base64
import json
import os
import sys
import time
from pathlib import Path

import anyio
from mcp import types
from mcp.server import Server
from mcp.server.stdio import stdio_server

from .capture import capture_png
from .cli import connect_agent
from .results import append_event, create_run, finalize, source_context

server = Server("magik2-framebuffer")


@server.list_tools()
async def list_tools() -> list[types.Tool]:
    return [
        types.Tool(
            name="capture_framebuffer",
            description=(
                "Capture the currently displayed MiSTer framebuffer as a PNG image. "
                "Does not navigate or restart the app. May install/update the native tooling service "
                "when capture support is missing. Raw pixels are the default; display derives a "
                "4:3 inspection view for 640x240/288 CRT rasters."
            ),
            inputSchema={
                "type": "object",
                "properties": {
                    "view": {
                        "type": "string",
                        "enum": ["raw", "display"],
                        "default": "raw",
                    }
                },
                "additionalProperties": False,
            },
        )
    ]


def capture_image(view: str) -> list[types.ImageContent | types.TextContent]:
    if not os.environ.get("MISTER_IP"):
        raise ValueError("MISTER_IP is required for framebuffer capture")
    started = time.monotonic()
    run = create_run(
        Path(os.environ.get("MISTER_MAGIK2_RESULTS", "build/magik2-results")),
        "capture-framebuffer",
        source_context(os.environ["MISTER_IP"]),
    )
    code = 1
    try:
        agent, _ = connect_agent(run, {"status", "capture-framebuffer"})
        fields, pixels = agent.capture_framebuffer()
        png, metadata = capture_png(fields, pixels, view)
        append_event(run, {"phase": "capture", **metadata})
        code = 0
        return [
            types.ImageContent(
                type="image",
                mimeType="image/png",
                data=base64.b64encode(png).decode("ascii"),
            ),
            types.TextContent(type="text", text=json.dumps(metadata, sort_keys=True)),
        ]
    finally:
        finalize(run, code, round((time.monotonic() - started) * 1000))


@server.call_tool()
async def call_tool(name: str, arguments: dict) -> types.CallToolResult:
    if name != "capture_framebuffer":
        return types.CallToolResult(
            isError=True, content=[types.TextContent(type="text", text="Unknown tool")]
        )
    try:
        content = await anyio.to_thread.run_sync(
            capture_image, arguments.get("view", "raw")
        )
        return types.CallToolResult(content=content)
    except (OSError, RuntimeError, ValueError) as error:
        return types.CallToolResult(
            isError=True, content=[types.TextContent(type="text", text=str(error))]
        )


def main() -> int:
    # Preserve one protocol descriptor; inherited build/bootstrap stdout belongs
    # on stderr too, otherwise automatic service installation corrupts MCP JSON.
    with os.fdopen(
        os.dup(sys.stdout.fileno()), "w", encoding="utf-8"
    ) as protocol_output:
        os.dup2(sys.stderr.fileno(), sys.stdout.fileno())

        async def serve() -> None:
            async with stdio_server(stdout=anyio.wrap_file(protocol_output)) as (
                reader,
                writer,
            ):
                await server.run(reader, writer, server.create_initialization_options())

        anyio.run(serve)
    return 0
