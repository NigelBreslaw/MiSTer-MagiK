# MiSTer Magic Main_MiSTer Fork

This directory is a project fork of upstream Main_MiSTer for MiSTer Magic
launcher experiments.

## Upstream

- Upstream project: https://github.com/MiSTer-devel/Main_MiSTer
- Imported baseline: `1f5337ee2cba8c62c361b1c044a2966b75c9ac67`
- Baseline date: `2026-06-01T01:18:27+08:00`
- Baseline subject: `CD-i: Added support for Subchannel RW (#1206)`
- License: GPL-3.0, preserved in `LICENSE`

This fork is not an official MiSTer-devel build. The deployed experiment binary
is named `MiSTer_Magic` so users can distinguish it from stock `/media/fat/MiSTer`.

## Why This Fork Exists

MiSTer Magic needs Main to remain the parent of core launch and hardware setup,
while the Slint GUI owns the product launcher experience. The first experiment
keeps the patch surface small:

- Start stock Main normally.
- Spawn `/media/fat/mister-magic/mister-magic-fb ui debug 86400` after menu init.
- Keep Main alive while Slint runs as a child process.
- Accept `mister_magic_launch <absolute-path>` on `/dev/MiSTer_cmd`.
- Route launch requests through Main's existing MRA/RBF loading path.

The main hypothesis is that Main-as-parent preserves NeoGeo SDRAM setup and
fixes launches such as Metal Slug 3 that fail when the GUI bypasses Main.

## Patch Map

- `support/mister_magic/mm_launcher.cpp`
- `support/mister_magic/mm_launcher.h`
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
- When publishing binaries, label them as MiSTer Magic builds, not official
  MiSTer-devel releases.

## Updating The Baseline

When refreshing from upstream:

1. Fetch or copy the new upstream Main_MiSTer tree.
2. Reapply the patch map above.
3. Update the baseline commit/date/subject in this file.
4. Build with `main-mister/build-docker.sh`.
5. Re-run the Main + Slint coexistence experiment on device.
