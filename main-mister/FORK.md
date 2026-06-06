# MiSTer MagiK Main_MiSTer Fork

This directory is a project fork of upstream Main_MiSTer for MiSTer MagiK
launcher experiments.

## Upstream

- Upstream project: https://github.com/MiSTer-devel/Main_MiSTer
- Imported baseline: `c73802332ff9c73659410084b6319ccd29f0b3aa`
- Baseline date: `2026-06-03T21:38:21+08:00`
- Baseline subject: `Release 20260603.`
- License: GPL-3.0, preserved in `LICENSE`

This fork is not an official MiSTer-devel build. The deployed experiment binary
is named `MiSTer_MagiK` so users can distinguish it from stock `/media/fat/MiSTer`.

## Why This Fork Exists

MiSTer MagiK needs Main to remain the parent of core launch and hardware setup,
while the Slint GUI owns the product launcher experience. The current fork keeps
the patch surface small and copies Zaparoo's launcher handoff model:

- Start stock Main normally through `/media/fat/MiSTer`.
- Use `[MiSTer] main=MiSTer_MagiK` for the update_all-compatible handoff.
- Require the repaired HDMI boot keys: `[MiSTer] direct_video=0`,
  `main=MiSTer_MagiK`, and `[Menu] video_mode=8`.
- Spawn `/media/fat/mister-magik/mister-magik-fb ui launcher 0` through
  `/sbin/agetty` on `tty2` after menu init.
- Keep Main alive while Slint runs as a child process.
- Accept `mister_magik_launch <absolute-path>` on `/dev/MiSTer_cmd`; this
  terminates the Slint child and routes launch through Main.

The previous direct-boot and custom OSD experiments are historical. Keep this
fork's production path close to Zaparoo unless a broader change is proven on
device.

## Patch Map

- `support/mister_magik/alt_launcher.cpp`
- `support/mister_magik/alt_launcher.h`
- `cfg.cpp`
- `scheduler.cpp`
- `user_io.cpp`
- `input.cpp`

Keep future changes similarly isolated unless a broader Main change is proven
necessary by a device experiment.

## Sync Etiquette

- Preserve upstream license and attribution.
- Keep local changes documented in this file and `docs/main-mister-fork.md`.
- Prefer small commits that can be rebased onto upstream Main_MiSTer.
- Do not vendor release artifacts from upstream; `releases/` is ignored.
- When publishing binaries, label them as MiSTer MagiK builds, not official
  MiSTer-devel releases.

## Updating The Baseline

When refreshing from upstream:

1. Find the latest upstream commit whose subject is `Release YYYYMMDD.` and
   whose diff updates `releases/MiSTer_YYYYMMDD`.
2. Copy that upstream tree into `main-mister/`, excluding `releases/`.
3. Reapply the patch map above.
4. Update the baseline commit/date/subject in this file.
5. Build with `main-mister/build-docker.sh`.
6. Re-run the Main + Slint coexistence experiment on device.
