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
- Escape-menu experiment: while the Slint child is active, `F12`/Menu yields
  the framebuffer route back to Main before forwarding the normal OSD key. If
  OSD closes or fails to open, Main restores Slint framebuffer routing.
  Normal logs only include OSD-relevant keys; create
  `/tmp/mister-magic-input-trace` on the device to log every `EV_KEY` code while
  the launcher child is active.

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
logs. Visual HDMI confirmation was completed on 2026-06-06: Metal Slug 3 boots
and runs correctly through Main-as-parent.

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

Escape-menu test:

- With the Slint debug child running, the Retro-bit/controller path generated
  `KEY_F12` (`code=88`) through Main's input path.
- Main classified it as a menu event and called the OSD-yield hook:

```text
user_io_kbd key=88 press=1 menu_event=1 visible=0
osd yield key=88 press=1 visible=0
user_io_kbd key=88 press=0 menu_event=1 visible=1
osd yield key=88 press=0 visible=1
```

This proves the fork can hand the framebuffer route back to Main and open the
OSD from the Slint child.

Follow-up overlay experiment: instead of handing the framebuffer route back to
Main, `KEY_F12`/Menu now toggles a tiny MiSTer Magic OSD directly through
`OsdWrite` while keeping Slint's buffer 0 active. The goal is to prove the OSD
plane can float over the Slint UI without the stock full-screen menu background,
CRT-static effect, or wallpaper flash. If this works visually, the next step is
to replace the placeholder rows with a navigable minimal escape menu.

Visual result: confirmed on HDMI. The MiSTer Magic OSD appears over the Slint
debug UI without the stock full-screen background. The Slint child still shows
only the debug scene (`ui debug 86400`, including the flashing
`Main alive + Slint child running` text); that is expected until the fork spawns
`ui launcher` instead.

Navigation follow-up: the overlay now has a tiny local state machine. While the
overlay is visible, D-pad/arrow keys move the selected row, Enter/Space closes
the placeholder action, and Menu/F12/Esc closes the overlay. Device logs confirm
`KEY_DOWN` (`108`), `KEY_UP` (`103`), and `KEY_ENTER` (`28`) reach this handler
and update the selected index.

Still to verify: visible highlight movement on HDMI, real actions for
Scripts/Input Mapping/Video/Reboot, and a deliberate product mapping for the
Retro-bit X/Menu button instead of relying on the current controller-generated
`KEY_F12` or `KEY_MENU` path.
