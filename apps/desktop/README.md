# MiSTer MagiK Desktop

`mister-magik-desktop` is a macOS-first Slint companion dashboard for MiSTer
MagiK. V1 is read-only: it shows agent, network, runtime, launcher, catalog, and
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

The default MiSTer host is `192.168.1.117`. Override it for development with:

```bash
MISTER_IP=192.168.1.50 scripts/dev-live.sh
```

## Agent Credentials

The app talks to the MiSTer MagiK agent on TCP `7498` with one JSON request per
line. It reads the agent token from:

1. `MISTER_AGENT_TOKEN`
2. `MISTER_AGENT_TOKEN_FILE`
3. `build/mister-agent.token` in the worktree root

Do not commit token files. OS keychain storage is intentionally left for a later
milestone.

The GUI uses the same token-protected line-delimited JSON protocol documented in
`docs/magik-agent.md`, but connects directly instead of shelling out through
`mister`. That keeps the desktop app cross-platform and avoids routing a
long-running UI through a Unix shell wrapper. Device scripts and recovery work
should still use `mister`.

## Analytics

The Analytics page can show the live framebuffer stream and a red 1px dirty-rect
overlay for recent keyframes and rect deltas reported by the agent stream.
One-shot PNG captures do not include dirty metadata, so they clear the overlay.

The Profile Artifacts panel imports local `MISTER_PROFILE_FILE` TSV output and
renders native frame-budget bars, dirty-region heatmaps, histogram/stat tables,
and slow-frame rows. Importing a TSV is read-only and local-only; the desktop app
does not run benchmark scripts or change MiSTer device state.

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
