# MiSTer MagiK

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
magik-gui/                         native frontend (mister-magik-fb)
  ui/launcher.slint           2×2 home grid + game launch
  ui/controller_test.slint    pad test scene
  src/launcher.rs             nav + fifo load_core game launch
  src/fpga.rs                 SPI, fb_enable_direct, set_vga_fb
main-mister/                       Main_MiSTer fork experiments
scripts/
  deploy-rust.sh              build + deploy Slint child binary
  deploy-main-mister-experiment.sh build + deploy Main-as-parent experiment
  install-slint-boot.sh       one-time: MiSTer.ini main= handoff to MiSTer_MagiK
  restore-stock-boot.sh       revert to stock MiSTer menu
  mister                      Rust SSH/status/snapshot wrapper
tools/mister/                 Rust host-side MiSTer CLI
history/                      experiment notes
AGENTS.md                     operational guide (read this for MiSTer quirks)
```

## Build & deploy

Requires [Docker](https://www.docker.com/) and [Rust](https://rustup.rs/).


See [`magik-gui/BUILD.md`](magik-gui/BUILD.md) for release profiles.
See [`docs/main-mister-fork.md`](docs/main-mister-fork.md) for the Main fork experiment.

## Boot into MiSTer MagiK

**Do not** set `main=mister-magik-fb` in `MiSTer.ini` — MiSTer execs away before
`video_init()` and the TV gets no HDMI signal.

Production boot: `/etc/inittab` keeps
booting stock `/media/fat/MiSTer`, and `[MiSTer] main=MiSTer_MagiK` hands off to
the Main_MiSTer fork. This preserves `update_all` and makes rollback simple.
Before editing, the scripts copy `/media/fat/MiSTer.ini` to
`/media/fat/MiSTer.ini.bak`. `MiSTer.ini` edits go through the Rust
comment-preserving mutator in `scripts/mister`: it installs
`[MiSTer] main=MiSTer_MagiK` and launcher-specific `[Menu]` video settings
without using ad hoc shell edits. Normal arcade direct-video output is configured
with `[arcade] direct_video=1`; vertical arcade stays scaler-based with
`[arcade_vertical] direct_video=0` and `video_mode=8` so MiSTer can rotate it.
The vertical section must appear after `[arcade]`, because MiSTer applies matching
sections in file order.

Build and deploy the fork + Slint child:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-main-mister-experiment.sh --fast
```

Or install the boot handoff after deploying binaries:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/install-slint-boot.sh
```

Restore stock menu by copying `/media/fat/MiSTer.ini.bak` back to
`/media/fat/MiSTer.ini` and ensuring `inittab` boots stock MiSTer:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/restore-stock-boot.sh
```

## Dev / manual run

Kill stock MiSTer so it releases the gamepad, then run the launcher:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/mister run \
  'killall MiSTer 2>/dev/null; sleep 1; /media/fat/mister-magik/mister-magik-fb ui launcher 60'
```

## Subcommands

| Command | Purpose |
|---|---|
| `ui [scene] [secs]` | Slint UI — default `launcher`; `secs=0` runs forever |
| `ui controller_test` | pad diagram test |
| `read` | SPI video mode / fb diagnostics |
| `fb` | geometry test pattern on HDMI |
| `input log\|sniff\|calibrate` | gamepad debugging |
| `audio-tone` | HDMI audio sanity check through `/dev/MrAudio` |
| `scenes` | list bench scene names |

## References

- [AGENTS.md](AGENTS.md) — MiSTer display model, gotchas, roadmap
- [Slint](https://slint.dev)
