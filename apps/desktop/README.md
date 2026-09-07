# MiSTer MagiK Desktop

`mister-magik-desktop` is a macOS-first Slint companion dashboard for MiSTer
MagiK. The interface is read-only: it shows agent, network, runtime, launcher, catalog, and
input status without rebooting, deploying, editing `MiSTer.ini`, launching cores,
or writing to `/dev/MiSTer_cmd`.

## Run

From this directory:

```bash
scripts/dev-live.sh
```

The script runs the app with:

- `SLINT_BACKEND=winit-skia` for Skia rendering.
- `SLINT_EMIT_DEBUG_INFO=1` for useful Slint debug metadata.
- Cargo features `live-ui,skia-renderer`.

Enable the embedded Slint MCP server only when needed:

```bash
scripts/dev-live-mcp.sh
```

That adds Cargo features `slint/mcp,live-ui,skia-renderer` and sets
`SLINT_MCP_PORT=9315`.
Slint 1.18's MCP feature currently pulls in the testing backend and software
renderer, so the default live loop keeps MCP off for faster Skia-only builds.

## Native connection and credentials

Desktop connects directly from Rust to the native service on TCP 7500. It uses
shared 2.0 device identity, remembered address and token storage, automatically
rediscovering a changed address. `MISTER_IP` is optional; there is no hard-coded
address or worktree-local credential file.

When installation or a missing capability requires it, Desktop invokes the
checkout's `scripts/magik desktop-prepare --json` once, then resumes direct TCP
communication. The helper uses the existing macOS Keychain SSH login. Different
compatible service builds are kept. Authentication/Keychain failures are shown;
there is no credential retry loop.

For multiple devices, select one explicitly with `scripts/magik device select
ADDRESS`. This milestone supports repository launch, not standalone bootstrap
packaging. See [the native API contract](../../magik/docs/desktop-api.md).

## Analytics

The Analytics page can show the live framebuffer stream and a red 1px dirty-rect
overlay for recent keyframes and rect deltas reported by the agent stream.
One-shot PNG captures do not include dirty metadata, so they clear the overlay.

The Profile Artifacts panel imports local `MISTER_PROFILE_FILE` TSV output and
renders native frame-budget bars, dirty-region heatmaps, histogram/stat tables,
and slow-frame rows. Importing a TSV is read-only and local-only; the desktop app
does not run benchmark scripts or change MiSTer device state.

Live Analytics retains CPU, memory, processes, network, storage, launcher FPS
and frame-phase timings. It no longer displays FPGA presentation/ownership/repeat
counters, over-budget counts, maximum frame times or vsync-miss streaks. Missing
measurements display as unavailable. Offline profile analysis is unchanged.

Still captures use authoritative latched scanout pixels. Live viewing uses the
application producer stream and is labelled separately; it does not prove which
frame the FPGA has displayed. Closing a view cancels its subscription. A transport
failure gets at most one rediscovery/reconnect; subsequent failure needs explicit
retry. PNG export and the existing local diagnostic modes use the same native
client.

## Slint UI Workflow

The default `live-ui` feature loads `ui/main.slint` at runtime via
`slint-interpreter`. When `ui/main.slint` changes, the running component exits
and reloads from disk, so UI edits do not require rebuilding the Rust side.

Fast UI checks:

```bash
scripts/check-ui.sh
slint-viewer --auto-reload ui/main.slint
slint-viewer --screenshot /private/tmp/mister-magik-desktop.png ui/main.slint
```

When started with `MISTER_DESKTOP_MCP=1`, the running app exposes Slint MCP at
`http://127.0.0.1:9315/mcp`; see `.mcp.json` for the local server definition.

## Verification

Useful local checks:

```bash
scripts/verify.sh
cargo test
cargo check --no-default-features --features compiled-ui
scripts/check-ui.sh
```

The `compiled-ui` feature keeps a build-time Slint path available for future
packaging, but V1 defaults to runtime-loaded UI for iteration speed.
Tests and coverage use Slint's software renderer by default so cold sandboxed
runs do not need the Skia prebuilt binary fetch.

For MCP smoke testing, run the app with `scripts/dev-live-mcp.sh` in one
terminal and then run `scripts/mcp-smoke.sh` in another.

Live streams start from a producer keyframe. They never take an automatic still
screenshot or use scanout pixels to seed producer deltas. If the application
producer is unavailable, Desktop reports that error without restarting MagiK.
