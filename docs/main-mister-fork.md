# Main_MiSTer Fork Experiment

`main-mister/` is our maintained Main_MiSTer fork. It is separate from
`magik-gui/` on purpose: Main owns hardware lifecycle and compatibility, while
Slint owns the product UI.

The fork provenance lives in `main-mister/FORK.md`. Current upstream baseline:

- Upstream: `MiSTer-devel/Main_MiSTer`
- Commit: `c73802332ff9c73659410084b6319ccd29f0b3aa`
- Date: `2026-06-03T21:38:21+08:00`
- Subject: `Release 20260603.`
- License: GPL-3.0, preserved in `main-mister/LICENSE`

This is not an official MiSTer-devel build. The experiment binary is deployed as
`MiSTer_Magic` so it is visually and operationally distinct from stock
`/media/fat/MiSTer`.

## Current experiment

- Main starts normally and initializes the menu core.
- If `/media/fat/mister-magic/mister-magic-fb` exists, Main spawns:

```text
/media/fat/mister-magic/mister-magic-fb ui debug 86400
```

- Main keeps polling while Slint is a child process.
- Main exposes an experimental command through `/dev/MiSTer_cmd`:

```text
mister_magic_launch <absolute-core-or-mra-path>
```

That command terminates the Slint child, drops the framebuffer route, and uses
Main's existing `xml_load` / `fpga_load_rbf` path. This is the path to test
Metal Slug 3 and the NeoGeo SDRAM setup bug.

## Patch map

- `support/mister_magic/mm_launcher.cpp`: Slint child lifecycle.
- `cfg.cpp`: forces framebuffer-terminal-friendly config for the experiment.
- `scheduler.cpp`: polls the launcher child.
- `user_io.cpp`: starts the launcher after the menu core is initialized.
- `input.cpp`: handles `mister_magic_launch`.

## Install model

The experiment deploys the fork as `/media/fat/MiSTer_Magic` and enables it with:

```text
main=MiSTer_Magic
```

Removing that line returns boot to stock `/media/fat/MiSTer`.

## Build

Prefer the Docker wrapper so the host does not need the ARM GCC installed:

```bash
main-mister/build-docker.sh
```

If `arm-none-linux-gnueabihf-gcc` is installed locally, plain `make -C
main-mister` also works. The experiment deploy script chooses the local compiler
when available and otherwise uses the Docker wrapper.

## Known limits

- This is not the final escape menu implementation.
- CRT support is not wired yet.
- Input ownership is intentionally left in the experimental state so we can see
  what Main, Slint, and the OSD do without over-designing the first pass.

## Release Baseline Policy

Main_MiSTer does not publish GitHub release tags. Its release markers are commits
named `Release YYYYMMDD.` that update `releases/MiSTer_YYYYMMDD`.

MiSTer Magic should pin `main-mister/` to one of those release commits, not an
arbitrary upstream `master` commit. The current baseline is `Release 20260603`
(`c73802332ff9c73659410084b6319ccd29f0b3aa`).

## 2026-06-06 Device Result

Experiment boot is now working on the test MiSTer:

- `/media/fat/MiSTer` boots with `main=MiSTer_Magic`.
- `/media/fat/MiSTer_Magic` initializes the menu core.
- Main starts `/media/fat/mister-magic/mister-magic-fb ui debug 86400` as a
  child process.
- The old inittab `mister-magic/boot.sh` handoff must be disabled for this
  experiment; the deploy script restores `::sysinit:/media/fat/MiSTer &`.
- `mister_magic_launch <path>` shuts down the Slint child and launches through
  Main.

Metal Slug 3 test:

```text
mister_magic_launch /media/fat/_Games/_Neo Geo MVS & AES/_Neo Geo Mister FGPA Ultra Pack/_ World A-Z/Metal Slug 3 (mslug3).mgl
```

Observed after launch:

```text
/media/fat/MiSTer /media/fat/_Console/NeoGeo_20250909.rbf ... Metal Slug 3 (mslug3).mgl
/tmp/CORENAME: NEOGEO
/sys/module/MiSTer_fb/parameters/mode: 8888 1 640 240 2560
```

No `Not enough memory` or `SDRAM config not found` strings appeared in device
logs. Visual HDMI confirmation is still required because the NeoGeo error is an
on-screen core message, not a normal Linux log line.

After `update_all` on 2026-06-06, the test MiSTer has stock Main/menu/core
release files from 20260603, including `NeoGeo_20260603.rbf`. The device's
`MiSTer.ini` had two unsupported VRR-specific keys, so they were commented with
MiSTer-style semicolon comments:

```text
;vrr_min_framerate=0
;vrr_max_framerate=0
```

The release-baseline fork was rebuilt and redeployed after `update_all`.
Retesting Metal Slug 3 through `mister_magic_launch` produced:

```text
/media/fat/MiSTer /media/fat/_Console/NeoGeo_20260603.rbf ... Metal Slug 3 (mslug3).mgl
/tmp/CORENAME: NEOGEO
/tmp/RBFNAME: NEOGEO
/sys/module/MiSTer_fb/parameters/mode: 8888 1 640 240 2560
```

No `Not enough memory` or `SDRAM config not found` strings appeared in logs.
