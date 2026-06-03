# mister-slint

A [Slint](https://slint.dev) front end for the **MiSTer FPGA**, built as a native
**Rust** binary with a custom software renderer, vsync pacing, and direct FPGA
framebuffer routing — no X11, Wayland, Zaparoo, or Python on device.

The headline result: **locked 60fps, smooth and tear-free** on HDMI. See
`history/2026-5-2/framebuffer-experiments.md` for how we got there.

```
+-------------------+   deploy binary    +---------------------------+
|  Dev machine      |  --------------->  |  MiSTer (armv7, glibc     |
|  cross + Docker   |                    |  2.31, /dev/fb0 1080p)   |
|  rust/build-arm   |  <--- capture fb --|  mister-magic-fb ui       |
+-------------------+                    +---------------------------+
```

## MiSTer device facts

| Property | Value |
|---|---|
| CPU / ABI | ARM Cortex-A9, `armv7l` hard-float (`gnueabihf`) |
| glibc | **2.31** (matches the cross container) |
| Display | `/dev/fb0`, 1920×1080, 32bpp (B,G,R,X); **no `/dev/dri`** |
| Framebuffer routing | FPGA SPI `video_fb_enable(1,0)` — ported in `rust/src/fpga.rs` |

## Project layout

```
rust/                         native frontend crate (mister-magic-fb)
  ui/app.slint                demo UI (animated bar + orbiting dots)
  src/main.rs                 subcommands: read | fb | ui
  src/fpga.rs                 FPGA SPI + fb_enable_direct
  src/fb.rs                   /dev/fb0 mmap, vsync, dirty-row copy
  build-arm.sh                cross build (--fast | --device)
  BUILD.md                    release vs release-device profiles
scripts/
  deploy-rust.sh              build + deploy (default: full release)
  mister_ssh.py               reliable paramiko SSH helper
  capture-fb.sh               grab /dev/fb0 → PNG
  raw_to_png.py               framebuffer dump → PNG (stdlib only)
  audit-mister.sh             device sanity check
history/                      experiment notes + screenshots
reference/                    gitignored MiSTer/Zaparoo source clones
```

## Build & deploy

Requires [Docker](https://www.docker.com/) (for `cross`), [Rust](https://rustup.rs/),
and [uv](https://docs.astral.sh/uv/) (host SSH tooling only).

Toolchain experiments: `scripts/bench-toolchain.sh` logs to
[`history/toolchain-bench/results.tsv`](history/toolchain-bench/results.tsv) (see
[`history/toolchain-bench/README.md`](history/toolchain-bench/README.md)).

```bash
# Full MiSTer release (~1.6 MB, fat LTO + NEON) — default
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh

# Fast daily build (~1.65 MB, ~3 min clean cross-compile)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh --fast

# See rust/BUILD.md for profiles and paths.
```

## Run on device

The stock menu owns the FPGA SPI bus, so pause it first (HDMI stays live):

```bash
MP=$(pidof MiSTer); kill -STOP $MP
/media/fat/mister-magic/mister-magic-fb ui 20   # animated demo, 20 seconds
kill -CONT $MP
```

Subcommands:

| Command | Purpose |
|---|---|
| `read` | dump live video mode / fb params (SPI diagnostics) |
| `fb [xoff] [yoff]` | paint geometry test pattern + route buffer 0 |
| `ui [secs]` | Slint demo UI, vsync-locked |

Remote via SSH helper:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py run \
  'MP=$(pidof MiSTer); kill -STOP $MP; /media/fat/mister-magic/mister-magic-fb ui 20; kill -CONT $MP'
```

## Verify what's on screen

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/capture-fb.sh build/shot.png
```

## Host tooling (Python)

Only **deploy/debug scripts** use Python on the dev machine — not the MiSTer app:

```bash
uv sync   # installs paramiko for mister_ssh.py
```

## Next steps

- Derive `xoff/yoff` from the live video mode (other resolutions / CRT)
- Wire controller/keyboard input
- Ship as a `main=` boot binary (Option C in AGENTS.md)

## References

- [Slint](https://slint.dev) — UI toolkit
- [AGENTS.md](AGENTS.md) — operational guide for agents/humans
- [Zaparoo](https://github.com/ZaparooProject) — prior art for MiSTer front ends
