# MiSTer-MagiK

A [Slint](https://slint.dev) front end for the **MiSTer FPGA**, built as a native
**Rust** binary with a custom software renderer, vsync pacing, and direct FPGA
framebuffer routing.

The headline result: **locked 60fps, smooth and tear-free** on HDMI. See
`history/2026-5-2/framebuffer-experiments.md` for how we got there.

## MiSTer device facts

| Property | Value |
|---|---|
| CPU / ABI | ARM Cortex-A9, `armv7l` hard-float (`gnueabihf`) |
| glibc | **2.31** (matches the cross container) |
| Display | `/dev/fb0`, 1920×1080, 32bpp (B,G,R,X); **no `/dev/dri`** |
| Framebuffer routing | FPGA SPI `SET_FBUF` + `set_vga_fb` — see `magik-gui/src/fpga.rs` |

## Project layout

```
magik-gui/                         native frontend (mister-magic-fb)
  ui/launcher.slint           2×2 home grid + game launch
  ui/controller_test.slint    pad test scene
  src/launcher.rs             nav + fifo load_core game launch
  src/fpga.rs                 SPI, fb_enable_direct, set_vga_fb
scripts/
  deploy-rust.sh              build + deploy binary + boot.sh
  install-slint-boot.sh       one-time: inittab → Slint launcher at boot
  restore-stock-boot.sh       revert to stock MiSTer menu
  mister-magic/boot.sh        on-device boot handoff script
  mister_ssh.py               paramiko SSH helper
history/                      experiment notes
AGENTS.md                     operational guide (read this for MiSTer quirks)
```

## Build & deploy

Requires [Docker](https://www.docker.com/), [Rust](https://rustup.rs/), and
[uv](https://docs.astral.sh/uv/) (host SSH only).


See [`magik-gui/BUILD.md`](magik-gui/BUILD.md) for release profiles.

## Boot into Slint (production)

**Do not** set `main=mister-magic-fb` in `MiSTer.ini` — MiSTer execs away before
`video_init()` and the TV gets no HDMI signal.

Instead, install the boot handoff once (MiSTer brings up HDMI, then Slint takes over):

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/install-slint-boot.sh
```

Restore stock menu:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/restore-stock-boot.sh
```

## Dev / manual run

Kill stock MiSTer so it releases the gamepad, then run the launcher:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py run \
  'killall MiSTer 2>/dev/null; sleep 1; /media/fat/mister-magic/mister-magic-fb ui launcher 60'
```

## Subcommands

| Command | Purpose |
|---|---|
| `ui [scene] [secs]` | Slint UI — default `launcher`; `secs=0` runs forever |
| `ui controller_test` | pad diagram test |
| `read` | SPI video mode / fb diagnostics |
| `fb` | geometry test pattern on HDMI |
| `input log\|sniff\|calibrate` | gamepad debugging |
| `scenes` | list bench scene names |

## Verify framebuffer (SSH)

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/capture-fb.sh build/shot.png
```

## References

- [AGENTS.md](AGENTS.md) — MiSTer display model, gotchas, roadmap
- [Slint](https://slint.dev)
