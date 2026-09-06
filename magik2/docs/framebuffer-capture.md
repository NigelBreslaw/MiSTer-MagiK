# Framebuffer screenshots directly into Codex

`scripts/magik2 mcp` serves one tool, `capture_framebuffer`, over MCP stdio.
A call returns a native `image/png` image block containing base64 PNG data, followed
by short JSON metadata. Codex receives the image directly; there is no screenshot
file, upload service or separate open-image step. Normal run metadata is retained
under `MISTER_MAGIK2_RESULTS`; pixels and base64 are never written to those logs.

## Connect Codex

Use Codex's [MCP configuration](https://developers.openai.com/codex/mcp/):

```sh
codex mcp add magik2-framebuffer --env MISTER_IP=192.168.1.117 -- /absolute/path/to/checkout/scripts/magik2 mcp
```

Use an absolute path to the checkout/worktree you intend to run. If that worktree
is removed, update the registration. Reconnect the MCP server in Codex after
changing its launch configuration. Do not add device passwords to tracked config.
The launch environment needs `uv` on PATH and the existing private Slint index
authentication when installing the host environment for the first time.

The server reuses 2.0's device token cache and capability-based connection flow.
When bootstrap is necessary, provide `MISTER_USER` and `MISTER_PASS` in its launch
environment, just as for ordinary 2.0 commands. A service without
`capture-framebuffer` is automatically built and installed through the existing
native update/bootstrap flow. This may take longer than a screenshot; no matching
branch, version or build hash is required if the capability is already present.
The screenshot operation itself does not start, restart, navigate or stop an app.

Call `capture_framebuffer` with `{}` for raw raster pixels, or
`{"view":"display"}` for an inspection view. Each call returns one image, not a
continuous feed. The server reserves stdout for MCP, including during automatic
build/bootstrap; diagnostics and inherited child-process output use stderr.

## Showing the image to the user

Receiving an MCP image proves delivery to the agent, not visibility in the chat.
When the user asks to see a capture, explicitly embed it in the assistant reply;
do not simply say "shown above" because the tool returned image content.

If the client does not expose the returned image as a reusable attachment, decode
that same base64 PNG into a temporary local file and embed its absolute path:

```markdown
![MiSTer MagiK framebuffer](/absolute/temporary/path/capture.png)
```

This optional presentation copy does not change the in-memory MCP capture path.
Do not capture again, navigate, restart the app, or commit screenshots just to
make an existing result visible. Keep agent receipt and user-visible presentation
as separate acceptance checks; a PNG decoding test only verifies the former.

## Pixels and presentation

Both real MagiK and Mini-MagiK are captured from the currently latched scanout slot.
The shared platform contracts provide the slot layout and FPGA status primitive.
The tool holds the platform transaction lock while reading and copying the active
slot, then checks that its geometry, sequence, base, flip count and route epoch
stayed stable and that MagiK still owns an enabled scanout.
There is no dependency on the legacy device agent, Slint screenshots, `watch-frame`
preview cache, or a raw `/dev/fb0` reader.

Raw is the default: RGB565 little-endian pixels become lossless RGB8 PNG pixels
using bit replication; row padding is excluded. A static framebuffer is valid.
For 640x240 and 640x288, `display` derives a 640x480 view using the existing
centred nearest-scanline convention: source row `((2*y+1)*height)//960`.
It never blends rows. All other geometries are unchanged. Metadata explicitly
identifies a derived transform, original and output sizes, stride and sequence.
This is framebuffer evidence, not a measurement of physical CRT/HDMI output.

## Native operation

The authenticated `capture-framebuffer` request takes no fields or binary body.
Its capability has the same name. A successful `framebuffer` response contains:

- `source`: `fpga-latched-scanout-slots`.
- `pixel_format`: `rgb565-le`.
- `width`, `height`, `stride_bytes`, `frame_sequence`.
- Binary body: exactly `stride_bytes * height` bytes.

Transport remains binary; PNG conversion and base64 encoding happen on the host.
The current shared scanout layout permits widths up to 1366, heights up to 768
and even strides up to 2736 bytes. Both ends validate geometry before using it.

Capture has one ten-second host deadline spanning connection and reception,
including a peer that keeps sending partial data. It is never automatically
retried. Bootstrap/service preparation happens before that deadline.

Errors distinguish `capture-unsupported`, `capture-unavailable`,
`capture-invalid-geometry`, `capture-invalid-request` and `capture-frame-changed`.
A changed frame can be requested again explicitly; it is never returned as a
successful potentially torn image. An unavailable scanout is an error, not a
reason to substitute a preview or restart the application. MCP failures contain
text and `isError`, with no fabricated image.

## Milestone 8 review

Focused validation covers RGB565 and padded stride, exact payload length,
geometry limits, both CRT heights, unchanged other geometries, no retry,
connect/partial-read deadlines, and SDK initialization/discovery/invocation.
The stdio fixture also checks that bootstrap subprocess output cannot corrupt
MCP and that invalid input/failures return errors without images.

Hardware acceptance used the already-installed applications; no application
rebuild, benchmark, profile or device reboot was needed. Real MagiK was restored
after the Mini-MagiK capture. Hardware evidence details are recorded below.

- Real MagiK: native 960x540 RGB565 scanout, sequence 163; MCP call 8.404 s,
  including the automatic service build/update. Image inspected directly in Codex.
- Mini-MagiK: native 960x540 RGB565 scanout, sequence 1; MCP call 2.958 s.
  Image inspected directly in Codex; the installed real app was then restored ready.
- With explicit approval for one additional capture, the real app's Arcade view
  was captured at 960x540, sequence 48. The Rust-painted rows and game preview
  were visible to the agent. The full MCP call took 11.291 s; this includes
  connection/preparation, so it is not a standalone capture-duration measurement.
  No navigation, restart or performance tuning was performed.
- Presentation correction: the tool image was not visible in the user's chat.
  The same returned PNG (86,157 bytes) was decoded into a temporary display copy
  and explicitly embedded in the assistant reply. No new capture was needed.
  Tool discovery guidance and scoped agent instructions now require this explicit
  presentation step when the user asks to see an image.

Local validation: 24 agent library tests; focused agent Clippy; typed ARM agent
build; 40 focused Python tests including real MCP stdio exchange. The unchanged
scope guard is run against the complete committed PR diff. CI owns broad checks.
The new dependency lock entries are four existing local Rust crates and the MCP
SDK's Python dependency graph; no previously locked packages were upgraded.
