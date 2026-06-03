# mister-magic

A proof-of-concept [Slint](https://slint.dev) UI, written with the **Python
bindings**, that runs on a **MiSTer FPGA** as a simple front end.

The first milestone is intentionally small: *show any Slint UI on the MiSTer*,
launched from the **Scripts** menu. It renders straight to the MiSTer
framebuffer (`/dev/fb0`) with Slint's software renderer - no X11, Wayland or GPU
involved.

```
+-------------------+        scp bundle        +--------------------------+
|  Dev machine      |  ----------------------> |  MiSTer (armv7, glibc    |
|  uv + Slint(py)   |                          |  2.31, /dev/fb0 1080p)   |
|  build bundle     |  <----- capture fb ----- |  Scripts -> mister-magic |
+-------------------+                          +--------------------------+
```

## Why this is not just `pip install slint`

Stock MiSTer Linux ships **Python 3.9** with no `pip`, while the Slint Python
wheels require **Python 3.12+**. The MiSTer is otherwise a great target: its
**glibc is 2.31**, which exactly matches Slint's `manylinux_2_31_armv7l` wheel,
and almost every native library the wheel needs (`libstdc++`, `libgcc_s`,
`libglib-2.0`, `libgobject-2.0`, `libffi`, `libexpat`, `libz`) is already on the
device.

So instead of touching the MiSTer's root filesystem, we ship a **self-contained
bundle** to the SD card:

- a portable **CPython 3.12** for `armv7-unknown-linux-gnueabihf`
  ([python-build-standalone](https://github.com/astral-sh/python-build-standalone),
  minimum glibc 2.17),
- the **Slint armv7l wheel**, whose vendored libraries
  (`libdrm`, `libgbm`, `libfontconfig`, `libfreetype`, `libinput`, `libudev`,
  `libxkbcommon`, ...) live in `slint.libs/` and are found via the extension's
  `$ORIGIN/../slint.libs` RPATH,
- a bundled **DejaVu** font + a minimal `fonts.conf`, because the MiSTer has no
  fonts and no fontconfig setup (otherwise text would not render).

## MiSTer device facts (audited)

| Property | Value |
|---|---|
| CPU / ABI | ARM Cortex-A9, `armv7l` hard-float (`gnueabihf`) |
| Kernel | `5.15.1-MiSTer` |
| Python (stock) | 3.9.6, no `pip` |
| glibc | **2.31** (matches the Slint wheel) |
| Display | `/dev/fb0`, 1920x1080, 32bpp (B,G,R,X); **no `/dev/dri`** |
| Video mode tool | `/usr/sbin/vmode` |
| RAM | ~492 MB total (~330 MB free) |
| SD card | mounted at `/media/fat` (lots of space) |

Re-run the audit any time:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/audit-mister.sh
```

## Project layout

```
mister-magic/
|-- ui/app-window.slint     # the UI (title, counter+button, colour bars)
|-- src/main.py             # entry point (loads the .slint, wires callbacks)
|-- pyproject.toml          # slint==1.16.1b1, requires-python >=3.12
|-- scripts/
|   |-- run-desktop.sh        # run on this machine via uv
|   |-- run-mister.sh         # on-device launcher (ships inside the bundle)
|   |-- build-arm-bundle.sh   # assemble build/mister-magic/ for the device
|   |-- deploy-mister.sh      # pack + scp + unpack on the MiSTer
|   |-- audit-mister.sh       # inspect the device over SSH
|   |-- capture-fb.sh         # grab /dev/fb0 -> PNG (see what's on screen)
|   `-- raw_to_png.py         # framebuffer-dump -> PNG converter
|-- deploy/mister-magic.sh  # MiSTer Scripts-menu entry -> /media/fat/Scripts/
`-- build/                  # generated bundle + downloads (gitignored)
```

## Develop on the desktop

Requires [uv](https://docs.astral.sh/uv/). All Slint Python releases are
pre-releases, so `pyproject.toml` opts in via `[tool.uv] prerelease = "allow"`.

```bash
uv sync                         # creates .venv with Python 3.12 + slint
scripts/run-desktop.sh          # opens the window
MISTER_MAGIC_CHECK=1 scripts/run-desktop.sh   # headless self-test (no display)
MISTER_MAGIC_SMOKE=1 scripts/run-desktop.sh   # open, render, auto-quit
```

## Build & deploy to the MiSTer

```bash
# 1. Assemble the ARM bundle (downloads CPython + wheel + font, ~100 MB).
scripts/build-arm-bundle.sh

# 2. Copy it to the MiSTer and install the Scripts-menu entry.
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-mister.sh
```

Then on the MiSTer: **OSD -> Scripts -> mister-magic**.

Installed on the device:

| Path | Purpose |
|---|---|
| `/media/fat/mister-magic/` | the self-contained bundle |
| `/media/fat/Scripts/mister-magic.sh` | the OSD Scripts entry |
| `/tmp/mister-magic.log` | runtime log (`/tmp` is wiped on power-off) |

The on-device launcher sets the framebuffer backend and font config:

```bash
SLINT_BACKEND=linuxkms-skia-software   # CPU rendering, no GPU
SLINT_BACKEND_LINUXFB=1                # use /dev/fb0 directly (no DRM/KMS)
FONTCONFIG_FILE=.../etc/fonts/fonts.conf
vmode -r 1920 1080 rgb32               # match the framebuffer
```

## Verify what's on screen

Since the dev machine can't see the MiSTer's HDMI output, dump the framebuffer:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/capture-fb.sh build/shot.png
```

## Known limitations / next steps

- **Input**: the MiSTer's `Main` binary owns USB input while it is in the menu,
  so the button may not be clickable from the Scripts launcher yet. The UI still
  renders. Proper input (keyboard/gamepad) is a follow-up.
- **Coexistence**: this runs as a Scripts utility while `Main` is alive; a full
  front end would later use the Zaparoo-style `main=` boot replacement.
- **Footprint**: the bundle is ~100 MB (mostly CPython + Skia). Fine for an SD
  card; could be trimmed (drop stdlib modules, strip) later.
- **Beta bindings**: Slint Python is beta; the version is pinned in
  `pyproject.toml` and `scripts/build-arm-bundle.sh`.

## References

- [Slint Python (PyPI)](https://pypi.org/project/slint/)
- [Slint LinuxKMS / linuxfb backend](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backend_linuxkms/)
- [python-build-standalone](https://github.com/astral-sh/python-build-standalone)
- [Zaparoo](https://github.com/ZaparooProject) - the MiSTer front end that inspired this approach
