# MiSTer MagiK Architecture

This document describes the current architecture. Dated experiments and older
attempts live in `history/`; treat those as evidence, not policy, unless this
document or `AGENTS.md` links to them as the reason for a current rule.

## Product Shape

MiSTer MagiK is a Rust/Slint frontend for MiSTer FPGA. It aims to make MiSTer
feel like a polished launcher rather than a configuration project: fast game
discovery, controller-friendly navigation, smooth 60fps browsing, and reliable
game launch.

The app repo contains the Rust/Slint frontend and deploy tooling. The
Main_MiSTer fork is maintained separately as `../Main_MiSTer` by default; see
`docs/main-mister-fork.md`.

## Boot And Process Model

Production boot stays compatible with stock MiSTer:

1. `/etc/inittab` starts stock `/media/fat/MiSTer`.
2. Stock Main reads `MiSTer.ini`.
3. `[MiSTer] main=MiSTer_MagiK` re-execs the MagiK Main fork.
4. The fork initializes HDMI/video through the normal Main path.
5. The fork runs `mister-magik-fb early-black` after `video_init()` so Rust
   clears and routes the launcher framebuffer before the full UI starts.
6. The fork starts `/media/fat/mister-magik/mister-magik-fb ui launcher 0` on
   `tty2` and enters dormant launcher mode.

The fork must not write the launcher framebuffer mode, route a generic 8888
framebuffer, draw the stock menu OSD, or keep input grabbed while Slint owns the
launcher.

## Framebuffer Ownership

The launcher path uses a small Linux framebuffer and FPGA scaling:

- Rust sets `/dev/fb0` to RGB565 for the launcher/UI path.
- Slint renders a 960x540 UI into cached RAM.
- The Rust frame loop copies dirty regions into the write-combined `/dev/fb0`.
- Rust sends the FPGA `SET_FBUF` route so buffer 0 is scanned to HDMI and scaled
  to the output mode.
- Slint periodically reasserts the route because other helpers can disturb the
  FPGA state without Main's bookkeeping noticing.

Important policy:

- Do not render Slint directly into the live framebuffer for production. The
  cached-RAM plus dirty-copy path is the current reliable 60fps design.
- Do not assume `/dev/fb0` contents are visible on HDMI. The FPGA may be scanning
  another buffer.
- RGB888 UI support has been removed. Keep any 8888 work in explicit low-level
  diagnostics such as framebuffer smoke/recovery tooling, not in app rendering
  or benchmarks.

Historical evidence:

- `history/2026-5-2/framebuffer-experiments.md`
- `history/2026-6-8/direct-fb-trial.md`
- `history/2026-6-9/direct-framebuffer-sidecar-retrospective.md`
- `history/2026-6-14/launcher-framebuffer-route-reassertion.md`

## Game Launch Handoff

Slint does not directly load cores. It hands launch requests back to Main so the
normal MiSTer loader path controls FPGA/core state and HDMI recovery.

Current command surface:

```text
mister_magik_launch <absolute .mgl/.mra/.rbf path>
mister_magik_exit_to_menu
```

Older fifo `load_core` flows may appear in history. Current work should prefer
the explicit fork command surface when operating under `MiSTer_MagiK`.

Never use external `rbf_load` from Slint; that path has historically left the
display without valid scan-out.

## Catalog And Preview Model

The runtime catalog is SQLite-backed. The UI should load from the database and
avoid scanning media during hot launcher paths.

Current rules:

- Build/update the library cache outside the UI hot path with
  `mister-magik-fb library-refresh` or `scripts/mister` helpers.
- Launcher boot loads the cached SQLite catalog before the first frame when a
  usable database exists, then performs refresh/preview validation in the
  background. Use `startup_timing` log lines to separate SQLite load, catalog
  construction, Slint bridge sync, and refresh costs.
- Rust launcher owns normal boot-time catalog validation. Main_MiSTer may invoke
  `library-refresh` only for the missing/empty DB first-boot deferral path and
  must not schedule delayed background refreshes when a database already exists.
- When the SQLite catalog is missing or empty, boot must start the Slint
  launcher immediately and let the launcher worker perform the first scan behind
  a visible full-screen scan state. Do not run foreground `library-refresh`
  before UI on first boot or after Reset Database; that regresses to a black
  HDMI screen while the index is built.
- The launcher presents a minimal `MiSTer MagiK` Slint splash immediately after
  `app.show()` and before catalog loading. Keep that path free of catalog,
  preview, media, or controller work so HDMI never sits on a black screen while
  the launcher warms up.
- Do not count helper payloads as games: BIOS ROMs, raw `.rbf` core binaries,
  menu-level computer/console launchers, and known support files are not normal
  launchables.
- Arcade and Neo Geo identities are keyed through MAME set names when metadata
  is available. See `history/2026-6-14/library-identity-model.md`.
- Preview availability comes from the preview archive index plus MRA `<setname>`
  virtual keys, not from walking PNG/JPG screenshot folders.
- Catalog code must not read `gamelist.xml`; runtime catalog loading goes
  through the SQLite library cache and materialized projections.
- MAME XML and MAME software-list XML are host-side metadata inputs only. Convert
  them to `mame.sqlite3` with `scripts/mister mame-metadata-build`; the runtime
  scanner consumes the SQLite rows, not those XML files.
- Runtime preview loading is raw565-oriented. Build cache assets and the
  compressed preview archive from the Mac with `tools/mister preview-cache-build`.

Relevant docs:

- `docs/console-media-identity.md`
- `history/2026-6-13/arcade-screenshot-cache-workflow.md`
- `history/2026-6-13/preview-zstd-archive-bench.md`
- `history/2026-6-14/library-scanner-preview-archive-pruning.md`
- `history/2026-6-14/mame-metadata-db.md`

## Build And Module Boundaries

`magik-gui/src/lib.rs` holds host-testable logic without the Slint/UI feature.
The binary target in `magik-gui/src/main.rs` owns device-only work: FPGA,
framebuffer, VT, input, audio, and the Slint runtime.

Use `magik-gui/BUILD.md` for build profiles, cross-compilation, FFmpeg, size
tracking, and CI details. Do not duplicate those details here.

## Open Areas

- Derive launcher geometry from live output mode rather than assuming the known
  stable 1080p HDMI path.
- Return to launcher after game reset without a full reboot.
- Continue controller mapping and hot-plug polish.
- Keep the fork patch surface small and documented in `../Main_MiSTer`
  `MAGIK_PATCHSET.md`.
