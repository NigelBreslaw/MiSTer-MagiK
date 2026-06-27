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

The launcher path uses a planned Linux framebuffer and FPGA scaling:

- Rust sets `/dev/fb0` to RGB565 for the launcher/UI path.
- Rust derives launcher geometry from `MiSTer.ini` menu output settings before
  startup drawing. 1080p-class outputs use a half-resolution framebuffer such as
  960x540; 720p, direct-video, and lower outputs run native.
- Slint renders the planned launcher framebuffer into cached RAM.
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
- RGB888/8888 UI support and color-route smoke tooling have been removed from
  the app. Do not add framebuffer format selection back to rendering,
  diagnostics, experiments, or benchmarks.

Historical evidence:

- `history/2026-5-2/framebuffer-experiments.md`
- `history/2026-6-8/direct-fb-trial.md`
- `history/2026-6-9/direct-framebuffer-sidecar-retrospective.md`
- `history/2026-6-14/launcher-framebuffer-route-reassertion.md`

## Game Launch Handoff

Slint does not directly load cores. It hands launch requests back to Main so the
normal MiSTer loader path controls FPGA/core state and HDMI recovery.

Launcher orchestration runs through an explicit Rust lifecycle state chart:

```text
BootSplash -> CatalogBuilding|CatalogReady -> Idle -> Launching -> Handoff|Recovered
Recovered -> Idle
```

`magik-gui/src/ui_runner/launcher_lifecycle.rs` owns lifecycle policy,
catalog-readiness state, launch staging, recovery transitions, and the small
effect stream that records bridge/render intent. `launcher_scheduler.rs` is the
central adapter for starting and polling catalog, media, launch, and background
jobs. The hot frame loop still owns Slint rendering, row-copy decisions,
framebuffer presentation, route reassertion, and frame accounting.
Lifecycle and scheduler internals should use explicit enum states for startup
readiness, pending launch refs, and worker availability instead of parallel
booleans or `Option` fields that can express impossible combinations.

Launch is intentionally two-phase. `Idle -> Launching` first updates the Slint
bridge and presents the loading frame. Only after
`loading_frame_presented(...)` does the scheduler let the existing
`LaunchHandoffSession` start the Main/FIFO handoff. If handoff fails, the
lifecycle enters `Recovered`, the scheduler reasserts the launcher framebuffer
route, the recovery UI is presented, and only then does
`recovery_frame_presented(...)` return the lifecycle to `Idle`.

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

See `docs/catalog.md` for the current catalog lifecycle, worker request modes,
root stamp semantics, SQLite publish model, and benchmark gates.

## Launcher Navigation Model

Arcade filter navigation is hierarchical. D-pad right descends into a filter
group or applies the highlighted value; `A` is the same action. D-pad left backs
out one filter level; `B` is the same action except at the filter top level,
where left is a no-op and `B` returns to the Home launcher. `Home` always jumps
back to the Home launcher from Arcade or any open filter level.

Arcade search is a top-level filter, but it behaves like its own mode instead
of another hierarchical drawer level. The left pane becomes an on-screen
keyboard and the Rust-painted game list moves to the right pane as search
results. Search results are cached as indexes into the hydrated
`ArcadeCatalog`; queries match normalized title text and the MRA basename,
including compact forms so punctuation and spaces do not block obvious matches.
While search is active, screenshot previews stay suppressed because the right
pane is reserved for result navigation. Launch return state stores the search
query and restores the filtered result list before selecting the returning game.

Current rules:

- Production `mister-magik-fb` exposes the minimal command surface:
  `ui`, `early-black`, `library-refresh`, and `experiment-capabilities`.
  Low-level probes are diagnostic/experiment builds, not release commands.
- Build/update the library cache outside the UI hot path with
  `mister-magik-fb library-refresh` or `scripts/mister` helpers.
- Launcher boot may seed Home/system counts from `library.summary.json` before
  the first usable frame when a usable database exists. Full SQLite row
  hydration then runs through the lifecycle scheduler after the first visible
  copy and configured warm-validation delay, without forcing a rebuild. If the
  stamp is stale, the launcher shows a `Library changed` dialog instead of
  rebuilding automatically. `Rebuild` runs the same full database builder used
  by explicit refresh; `Continue` writes a one-shot marker so the next MagiK
  boot goes directly to the `Updating Library` rebuild screen. Use
  `startup_timing` log lines to separate summary seed, full SQLite hydration,
  catalog construction, Slint bridge sync, stamp check, user choice, and build
  costs.
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
- Preview requests use derived archive paths and identity keys. The catalog does
  not index screenshot archives or walk PNG/JPG screenshot folders; missing
  preview entries fail at runtime and show the blank preview state.
- Catalog code must not read `gamelist.xml`; runtime catalog loading goes
  through the SQLite library cache and materialized projections.
- MAME XML and MAME software-list XML are host-side metadata inputs only. Convert
  them to `mame.sqlite3` with `scripts/mister mame-metadata-build`; the runtime
  scanner consumes the SQLite rows, not those XML files.
- Runtime preview loading is raw565-oriented. Build source screenshots, raw565
  caches, fixed LZ4-block `.mmlz4b` preview packs, and `.mmlz4b.idx` seek
  sidecars from the private `private/magik-cloud` submodule. The sidecar is a
  first-preview latency optimization; the whole pack remains the correctness
  fallback. Runtime deploy does not build catalog/media artifacts. Use
  `scripts/magik-cloud path` when a command needs to resolve the checkout.

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

- Return to launcher after game reset without a full reboot.
- Continue controller mapping and hot-plug polish.
- Keep the fork patch surface small and documented in `../Main_MiSTer`
  `MAGIK_PATCHSET.md`.
