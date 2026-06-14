# AGENTS.md — mister-slint

Operational guide for AI agents (and humans) working on this project. Read this
first. It captures the goal, the hard-won knowledge about how the MiSTer
displays a Linux UI, the tooling we built, and the concrete next steps.

> Keep this file current. When you learn something non-obvious about the MiSTer,
> Slint, or the deploy path, add it here rather than re-deriving it next session.

---

## 1. Goal & current status

**Goal:** run a [Slint](https://slint.dev) UI on a **MiSTer FPGA** as a simple
game/app front end. Long-term it should feel like a real frontend (own the
screen, controller input).

**Status (2026-06-08):**

- ✅ **Native Rust frontend** — `mister-magik-fb ui` at locked 60fps, smooth +
  tear-free. See §9.7.
- ✅ **FPGA-scaled small framebuffer UI path** — benchmarks and launcher
  temporarily set `/dev/fb0` to 960×540, render Slint 1:1 into that
  write-combined buffer, and let MiSTer's FPGA framebuffer scaler expand it to
  1080p HDMI. The old ARM-side 960×540 → 1920×1080 framebuffer copy/scaler has
  been removed from the normal UI path.
- ✅ **Slint launcher** — 2×2 home grid, controller test; gamepad owned by Slint
  while UI runs. Games launch via fifo `load_core` (MiSTer spawned briefly).
- ✅ **Production boot** — update_all-compatible handoff:
  `/etc/inittab` boots stock `/media/fat/MiSTer`, then `[MiSTer]
  main=MiSTer_MagiK` re-execs the Main fork, which launches Slint as its child.
- ✅ **Slint-owned MagiK framebuffer handoff** — Main initializes HDMI, then
  Rust clears and routes `/dev/fb0` with the selected `MISTER_FB_FORMAT`
  before the full Slint UI starts. Main no longer writes the MagiK launcher
  framebuffer mode or generic 8888 route while Slint is active.
- ✅ **HDMI boot verified** — `MiSTer.ini` is edited through the Rust
  comment-preserving mutator, with `[MiSTer] main=MiSTer_MagiK` and
  launcher-specific `[Menu] direct_video=0` / `video_mode=8`. Keep global
  `[MiSTer] direct_video=0` for launcher boot; put arcade original-timing output
  under `[arcade] direct_video=1`.
- ✅ FPGA SPI + `fb_enable_direct` + `set_vga_fb` in `magik-gui/src/fpga.rs`.
- ✅ Gamepad input via Linux js API (`magik-gui/src/input.rs`).

See `history/2026-5-2/framebuffer-experiments.md` for the exploration path.

Remaining work: in-game settings strategy (§7).

### Canonical names

- Product/UI text: **MiSTer MagiK**.
- Main_MiSTer fork binary/process/device path: `MiSTer_MagiK`.
- Slug for directories and scripts: `mister-magik`.
- Slint framebuffer binary/package: `mister-magik-fb`; Rust crate/import name:
  `mister_magik_fb`.
- Do not introduce legacy mixed-case variants or the old `magic` spelling in
  operational code, paths, scripts, or docs.

---

## 2. The MiSTer device (facts)

- Host on the LAN: `192.168.1.117`, SSH `root` / password `1`.
- CPU/OS: ARM Cortex-A9 **armv7l**, minimal Linux, **glibc 2.31**. DE10-Nano,
  **1 GiB DDR3** (≈400 MB free in our tests).
- Framebuffer: `/dev/fb0`, driver **`MiSTer_fb`** (`/proc/fb` → `0 MiSTer_fb`).
  Size follows the active MiSTer mode; byte order is **B,G,R,X** for
  `rgba 8/16,8/8,8/0` 32-bit modes (little-endian). No `/dev/dri` (no DRM/KMS).
- Audio: ALSA exposes only `Dummy PCM` on this device. HDMI-capable Linux-side
  audio is `/dev/MrAudio` (`/sys/devices/virtual/MrAudio_sys/MrAudio/dev` →
  `246:0`), backed by the MiSTer kernel audio buffer.
- FB mode is set by writing `/sys/module/MiSTer_fb/parameters/mode` as
  `"<fmt> <rb> <width> <height> <stride>"`, e.g. `8888 1 1920 1080 7680`.
- No system fonts on MiSTer — the Rust build embeds Press Start 2P in the binary
  (`magik-gui/ui/fonts/`, SIL OFL).
- Production `MiSTer.ini` keys for MagiK: `[MiSTer] main=MiSTer_MagiK`,
  `[MiSTer] direct_video=0`; `[Menu] direct_video=0`, `video_mode=8`;
  `[arcade] direct_video=1`; `[arcade_vertical] direct_video=0`,
  `video_mode=8`, `vscale_mode=1`.
- `MiSTer.ini` is parsed top-to-bottom. Because vertical arcade games match both
  `[arcade]` and `[arcade_vertical]`, keep `[arcade_vertical]` after `[arcade]`
  so the vertical 1080p/non-direct override wins.
- `MiSTer.ini` has a backup at `/media/fat/MiSTer.ini.bak`.
- `/media/fat` is exFAT (via FUSE). Writing many small files is **slow**; that
  dominates deploy time, not the network.

---

## 3. How the MiSTer puts a Linux UI on HDMI (the important part)

This is the knowledge that took the most effort. Source references are to the
cloned repos under `reference/` (see §6).

### 3.1 Why writing `/dev/fb0` is invisible at the menu

The MiSTer HPS framebuffer has **multiple buffers** spaced `FB_SIZE` apart.
`/dev/fb0` is **buffer 0**. The FPGA scans out **exactly one** buffer to HDMI,
selected by an FPGA SPI command (`UIO_SET_FBUF`). See
`reference/Main_MiSTer/video.cpp:3284` `video_fb_enable(int enable, int n)`:

```c
spi_w(FB_EN | FB_FMT_RxB | FB_FMT_8888); // enable + format
spi_w(fb_addr low); spi_w(fb_addr high); // which buffer (n)
spi_w(fb_width); spi_w(fb_height); ...   // dims + scaled rect + stride
```

At the **main menu**, the code deliberately keeps the FPGA pointed at the
**wallpaper buffer**, not buffer 0:

```c
if (is_menu() && !enable && menu_bg) { enable = 1; n = menu_bgn; } // video.cpp:3293
```

So the wallpaper you see is the menu's wallpaper **buffer
`menu_bgn`**, drawn by the main binary (`draw_*`/`video_menu_bg` in `video.cpp`).
**Buffer 0 (`/dev/fb0`) is never scanned at the menu.** Our Slint app writes
buffer 0 correctly (the PNG dumps prove it) but the FPGA isn't looking at it.

**Empirical confirmation:** we paused the menu process (`kill -STOP $(pidof
MiSTer)`) and the Slint frame *still* didn't appear on HDMI — ruling out simple
"menu paints over us" contention. The framebuffer dump showed Slint; the screen
did not. Only `video_fb_enable(1, 0)` (FPGA SPI, main-binary only) makes buffer
0 visible.

### 3.2 The `main=` boot hook is stock MiSTer

`main=` is a **native** MiSTer.ini option:

- `reference/Main_MiSTer/cfg.cpp:141` maps INI key `MAIN` → `cfg.main`.
- `cfg.cpp:612` default is `cfg.main = "MiSTer"`.
- `reference/Main_MiSTer/user_io.cpp:1463` re-execs the configured binary.
- Stock `reference/Main_MiSTer/MiSTer.ini:381` documents `;main=some_binary_file`.

Setting `main=some/path/to/binary` makes `/media/fat/MiSTer` re-exec that binary
at boot instead of the stock menu. Use this for the Main fork
(`main=MiSTer_MagiK`), not for the Slint binary (`main=mister-magik-fb`), because
the Slint binary runs before `video_init()` and the TV gets no HDMI signal (§7).

### 3.3 Engineering takeaway

To show Slint on HDMI we need:

1. **Rust-owned `SET_FBUF`** — `magik-gui/src/fpga.rs` points scan-out at
   buffer 0 with the Slint-selected format/stride.
2. **Correct launcher HDMI mode** — the launcher path uses global `[MiSTer]
   direct_video=0`, plus `[Menu] direct_video=0` and `[Menu] video_mode=8`.
   Normal arcade games use `[arcade] direct_video=1`; vertical arcade games use
   `[arcade_vertical] direct_video=0` and `video_mode=8` because the external
   scaler cannot rotate direct-video timings.
3. **A render loop** — custom Slint `Platform`, vsync pacing, dirty-row copy into
   write-combined `/dev/fb0` (§9.7).

The Main fork spawns Slint through `agetty` on `tty2`, switches to VT2, releases
Main's input grab, suppresses the stock OSD/menu path, and leaves framebuffer
mode/routing to Rust. After `video_init()`, Main runs
`mister-magik-fb early-black` once; that helper writes the selected
`/dev/fb0` mode, clears a black frame, and routes buffer 0. The full Slint child
then repeats the same ownership step before drawing UI. Slint also has Rust SPI
diagnostics (`magik-gui/src/fpga.rs`) for reading/reasserting framebuffer state.
Games launch by spawning MiSTer briefly and writing fifo `load_core` (§7). Do
**not** use external `rbf_load` from Slint — bricks HDMI.

**F9 / keep-MiSTer-running was tried and rejected:** MiSTer keeps `EVIOCGRAB`
and the FPGA menu OSD unless it processes F9 internally. Injected F9 from uinput
is unreliable; coexistence gave stock menu on HDMI with no Slint input.

---

## 4. Repo layout (this project)

```
scripts/
  deploy-rust.sh            build + deploy Slint child binary
  install-slint-boot.sh     one-time MiSTer.ini main=MiSTer_MagiK handoff
  restore-stock-boot.sh     revert to stock MiSTer menu
  mister               Rust SSH/status/snapshot wrapper
  audit-mister.sh       device sanity check (+ Cortex-A9 / NEON cpuinfo for A1)
tools/mister/          Rust host-side MiSTer SSH + observability CLI
reference/              READ-ONLY clones (gitignored) — see §6
build/                  gitignored framebuffer PNG dumps
history/                experiment notes (framebuffer-experiments.md, screenshots)
magik-gui/                   native armv7 frontend — see §12
  ui/launcher.slint     home grid + embedded controller test
  ui/controller_test.slint
  ui/bench/*.slint      perf bench scenes
  ui/fonts/PressStart2P-Regular.ttf
  src/main.rs           read | fb | ui | input | scenes
  src/launcher.rs       nav state machine + launch_mra
  src/fpga.rs           SPI + fb_enable_direct + set_vga_fb
  src/fb.rs             /dev/fb0 mmap, vsync, dirty-row copy, boot retry
  build-arm.sh          ARM build entrypoint (Apple container locally, cross in CI); see magik-gui/BUILD.md
  BUILD.md              ARM build profiles
main-mister/            buildable Main_MiSTer fork experiments — see docs/main-mister-fork.md
  build-docker.sh       Dockerized Main build with ARM GCC 10.2
```

On the device the binary lives at `/media/fat/mister-magik/mister-magik-fb`.

---

## 5. Workflow & commands

Always use `scripts/mister` for device comms. It is the Rust SSH/status wrapper
and keeps the password-auth behavior reliable without raw `ssh`/`scp`.
The wrapper and workflow scripts default to `MISTER_IP=192.168.1.117` and
`MISTER_PASS=1`, so run them without inline env assignments unless you are
intentionally targeting a different MiSTer.

```bash
# Build + deploy (default = release-device / A3, ~5.6 MiB)
scripts/deploy-rust.sh

# Build only: magik-gui/build-arm.sh  |  magik-gui/build-arm.sh --device
# Profiles: magik-gui/BUILD.md

# Fast host checks (do not compile Slint/AppKit)
scripts/dev-rust fmt
scripts/dev-rust test
scripts/dev-rust check
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings

# One-time: boot into MiSTer_MagiK through stock MiSTer main= handoff
scripts/install-slint-boot.sh

# Re-deploy binary + fork after code changes
scripts/deploy-rust.sh

# Restart the already-deployed Rust UI with no build/copy
scripts/run-rust.sh arcade 0
scripts/run-rust.sh launcher 0

# Prefer scripts/run-rust.sh for dev runs; it stops stale owners and starts the
# already-deployed binary without rebuilding or copying.

# Build/update the SQLite library cache outside the UI hot path. Production
# MiSTer_MagiK launcher scripts block for the first missing cache, then delay
# later background refreshes until after the Slint UI has had time to settle; the
# UI process refreshes in-process only with MISTER_CATALOG_REFRESH=1.
scripts/mister run "/media/fat/mister-magik/mister-magik-fb library-refresh"

# Inspect the SQLite library cache directly on the MiSTer. The device does not
# ship sqlite3(1); use the MagiK binary's bundled rusqlite support instead of
# pulling /media/fat/mister-magik/library.sqlite3 back to the host.
scripts/mister db
scripts/mister db "SELECT count(*) FROM games"
scripts/mister db "SELECT system_id, count(*) FROM games GROUP BY system_id ORDER BY count(*) DESC LIMIT 20"

# Restore stock MiSTer menu boot
scripts/restore-stock-boot.sh

# Temporary comparison boot: switch main= to the installed Zaparoo fork/frontend
# at /media/fat/zaparoo/{MiSTer_Zaparoo,frontend}, then reboot.
scripts/mister ini-zaparoo-boot
scripts/mister reboot-wait

# Switch back to MiSTer MagiK production boot.
scripts/install-slint-boot.sh
scripts/mister reboot-wait

# Device comms
scripts/mister run "uname -a"
scripts/mister reboot
scripts/mister reboot-wait
scripts/mister wait

# Toolchain A/B (host build + all ui/bench scenes on device) → history/toolchain-bench/results.tsv
scripts/bench-toolchain.sh A0 --clean --replace-label
```

Library scan benchmarks include a cached no-change manifest check by default.
Set `MISTER_LIBRARY_BENCH_CHANGED_REFRESH=1` only on disposable roots when you
need changed-refresh timings; it creates a synthetic candidate file under the
first configured library root before measuring the rebuild path.

`bench-toolchain.sh` keeps the historical TSV schema, but new rows append
display metadata to `notes`: `ini_mode`, `physical_mode`, `fb_size`,
`render_size`, `fb_scale`, `pixel_repetition`, and `uio_fb`.
Display-mode visual sweep records live in
`history/toolchain-bench/display-modes-20260607.md`.

### Agent sandbox / approval hygiene

Codex sandbox approval matches the outer command prefix. Inline environment
assignments (`MISTER_IP=... scripts/...`) and shell wrappers (`zsh -lc ...`) make
otherwise-approved device commands look like new, unapproved commands. To avoid
the common "Operation not permitted, rerunning with approval" loop:

- Prefer direct commands such as `scripts/mister ...`,
  `scripts/deploy-rust.sh`, and `scripts/bench-toolchain.sh ...`.
- When device/network work is required, request escalation on the direct workflow
  command the first time, with a scoped prefix like `scripts/mister`,
  `scripts/deploy-rust.sh`, `scripts/deploy-main-mister-experiment.sh`, or
  `scripts/bench-toolchain.sh`.
- Do not use raw `ssh`/`scp`, and avoid `/bin/zsh -lc` wrappers for normal MiSTer
  operations.

Bench scenes: `mister-magik-fb scenes` or `ui <scene> 20` — see `magik-gui/ui/bench/README.md`.
Log: [`history/toolchain-bench/README.md`](history/toolchain-bench/README.md).

**Arcade benchmark hygiene:** do **not** use row-by-row/stepwise list scenarios
(`list-scroll`, old `smooth-scroll`, or manual selected-index jumps) for arcade
performance conclusions. They do not reproduce the real visual workload and keep
leading us to the wrong optimizations. Use velocity-scroll scenarios instead:
`held-scroll` for normal continuous motion, `turbo-hold` for fast continuous
motion, and `scripts/profile-preview-scroll.sh` / `scripts/profile-arcade-scroll.sh`
for preview/list benchmark work. `velocity-scroll` is an alias for `held-scroll`.
Do not reintroduce the old live-framebuffer arcade scroll-present path
(`MISTER_ARCADE_SCROLL_PRESENT` / `--scroll-present`): a 2026-06-11 MiSTer A/B
showed it roughly doubled framebuffer present cost versus the normal cached-RAM
full-list copy. See `history/2026-6-8/arcade-band-copy-trial.md`.
Use RGB565 for production arcade performance conclusions. `MISTER_FB_FORMAT=8888`
is retained for diagnostic framebuffer/color-route A/B runs, but a 2026-06-14
held-scroll check measured RGB565 at p95 17.1 ms with 1 frame >20 ms while 8888
measured p95 28.7 ms with 150 frames >20 ms over the same 10 s run.
Screenshot transitions live on the real `ui arcade` surface. Default preview
changes use `fade`; use `scripts/profile-preview-transition-mega.sh LABEL
--deploy-device` to run every raw-preview transition and summarize the trace by
`transition_effect`. Add new transition experiments as additional
`MISTER_PREVIEW_TRANSITION` names instead of replacing existing effects. For
visual review, use `MISTER_LAUNCHER_BENCH_SCENARIO=preview-step-hold`.
Arcade screenshot cache update workflow is documented in
`history/2026-6-13/arcade-screenshot-cache-workflow.md`: originals live in
`/media/fat/_Arcade/media/screenshot`, generated caches live under
`/media/fat/_Arcade/media/screenshot-magik`, and only cache directories should be
deleted/recreated. MiSTer MagiK runtime preview loading is raw565-only; build
and deploy `.rgb565` caches from the Mac with `tools/mister preview-cache-build`
instead of reintroducing device-side decode/resize/cache-writing paths.
Single-file preview-cache benchmark results live in
`history/2026-6-13/preview-zstd-archive-bench.md`. The locked-in real arcade path
auto-detects a sibling raw565 raw pack (`raw565-<cache>-rawpack.mmraw`), preloads
it into RAM, and falls back to the smaller LZ4 block archive
(`raw565-<cache>-lz4block-12.mmlz4b`) or individual `.rgb565` files. Latest
MiSTer turbo-scroll + fade measurements: separate files loaded previews in
~10.2ms each; the auto raw pack loaded them in ~0.54ms each with the same
one-placeholder behavior and vsync-bound frame pacing. The raw pack is large
(~147.7MB for the hybrid 320x320 cache); the LZ4 archive stays small (~19.4MB)
and measured ~1.6ms per preview in the same real-app path.
The library scanner must not read `gamelist.xml`, walk screenshot/cache media
directories, or probe normal PNG/JPG screenshot files for preview metadata.
Preview availability comes from the compressed preview archive index plus MRA
`<setname>` virtual keys. See
`history/2026-6-14/library-scanner-preview-archive-pruning.md`.
Classic full-screen camera/background effect experiments live in
`ui camera-effects`; list labels with `mister-magik-fb camera-effects`, browse
interactively with `scripts/run-rust.sh camera-effects 0`, and benchmark with
`scripts/profile-camera-effects.sh LABEL --mode mega --segment-secs N`. The
trace includes CPU plus `clear/background/projection/image_blit/sprite/post/hud`
timing buckets. Baseline results append to
`history/toolchain-bench/results-camera-effects.tsv`; see
`history/2026-6-13/camera-effects-catalog.md`.
Classic full-screen sprite/object effect experiments live in
`ui sprite-effects`; list labels with `mister-magik-fb sprite-effects`, browse
interactively with `scripts/run-rust.sh sprite-effects 0`, and benchmark with
`scripts/profile-sprite-effects.sh LABEL --mode mega --segment-secs N`. The trace
uses the camera-effect timing buckets and adds `sprite_count`, `sprite_pixels`,
`particle_count`, and `flicker_skip_count`. Baseline results append to
`history/toolchain-bench/results-sprite-effects.tsv`; see
`history/2026-6-13/sprite-effects-catalog.md`.
Classic full-screen arcade/game and Amiga demo text effect experiments live in
`ui text-effects`; list labels with `mister-magik-fb text-effects`, browse
interactively with `scripts/run-rust.sh text-effects 0`, and benchmark with
`scripts/profile-text-effects.sh LABEL --mode mega --segment-secs N`. The trace
uses the camera-effect timing buckets and adds `glyph_count`, `glyph_pixels`,
`tile_count`, `vector_segment_count`, `bob_count`, `palette_step_count`,
`hidden_glyph_count`, and `scroll_offset`. Baseline results append to
`history/toolchain-bench/results-text-effects.tsv`; see
`history/2026-6-13/text-effects-catalog.md`.
Classic full-screen raster/palette effect experiments live in
`ui raster-effects`; list labels with `mister-magik-fb raster-effects`, browse
interactively with `scripts/run-rust.sh raster-effects 0`, and benchmark with
`scripts/profile-raster-effects.sh LABEL --mode mega --segment-secs N`. The trace
uses the camera-effect timing buckets and adds `palette_step_count`,
`lut_lookup_count`, `row_op_count`, `dither_pixel_count`, `flash_pixel_count`,
`trail_pixel_count`, `indexed_pixel_count`, and `reflection_row_count`. Baseline
results append to `history/toolchain-bench/results-raster-effects.tsv`; see
`history/2026-6-13/raster-effects-catalog.md`.
Classic full-screen screen/transition effect experiments live in
`ui transition-effects`; list labels with `mister-magik-fb transition-effects`,
browse interactively with `scripts/run-rust.sh transition-effects 0`, and
benchmark with `scripts/profile-transition-effects.sh LABEL --mode mega
--segment-secs N`. The trace uses the camera-effect timing buckets and adds
`mask_cell_count`, `revealed_pixel_count`, `hidden_pixel_count`,
`source_a_pixel_count`, `source_b_pixel_count`, `shake_offset_px`,
`flash_pixel_count`, `warp_sample_count`, `ghost_pixel_count`, and
`glitch_band_count`. Baseline results append to
`history/toolchain-bench/results-transition-effects.tsv`; see
`history/2026-6-13/transition-effects-catalog.md`.

**Debug trick — see Slint without HDMI routing:** dump `/dev/fb0` and convert to
PNG **while `mister-magik-fb ui` is running**. After exit, fbcon shows `login:` and
the dump is useless. `bench-toolchain.sh` snapshots at ~`scene_secs - 2` s mid-run.

---

## 6. Reference source (cloned, read-only, gitignored)

Cloned under `reference/` (shallow). For study only; do not edit/commit.

| Dir | Repo | Why it matters |
|-----|------|----------------|
| `reference/MiSTer-devel_Main_MiSTer` | `MiSTer-devel/Main_MiSTer` | **Upstream baseline** for fork diffs. Compare against `Main_MiSTer` to see the minimum alt-launcher patch surface. |
| `reference/Main_MiSTer` | `ZaparooProject/Main_MiSTer` (fork) | **`video.cpp`** (`video_fb_enable` :3284), **`fpga_io.cpp`/`spi.cpp`** (SPI bit-bang), **`cfg.cpp`** (`main=` hook). **`support/zaparoo/alt_launcher*.cpp`** — external frontend spawn/coexistence. **`ZAPAROO_FORK.md`** change map. |
| `reference/cybermobile_Main_MiSTer` | `cybermobile/Main_MiSTer` | In-process **`gfx_menu.cpp`** (~3k lines, Imlib2) reskins the stock menu; toggled via `gfx_menu_enable` in INI. No separate process. |
| `reference/zaparoo-launcher` | `ZaparooProject/zaparoo-launcher` | Qt/QML **`zaparoo/frontend`** binary that `alt_launcher.cpp` execs on **tty2**. Rust Core client via cxx-qt. |
| `reference/mister-companion` | `Anime0t4ku/mister-companion` | Historical reliable MiSTer SSH reference. The active wrapper is now Rust (`scripts/mister`). |

Optional clones (historical — explored other front ends, **not used by this project**):
`Menu_MiSTer`, `zaparoo-frontend`, `zaparoo-core`, `Zaparoo_MiSTer`.

Key files: `reference/Main_MiSTer/video.cpp` (around `video_fb_enable`),
`fpga_io.cpp`, `user_io.h` (UIO/FB constants).

To refresh: `git -C reference/<repo> pull` (or re-clone `--depth 1`).

---

## 7. Architecture & roadmap

### Current — production boot + dev binary

**Production:** use `main=` handoff, not direct inittab
replacement. `/etc/inittab` must keep stock `/media/fat/MiSTer` as the menu
entry. `[MiSTer] main=MiSTer_MagiK` in `MiSTer.ini` makes stock MiSTer re-exec
the Main fork, and the fork launches
`/media/fat/mister-magik/mister-magik-fb ui launcher 0` as its child.

This keeps `update_all` first-class: stock `/media/fat/MiSTer` remains the
canonical boot binary, while `/media/fat/MiSTer_MagiK` is an add-on payload that
can later be managed by update_all. Rollback is simple: remove
`main=MiSTer_MagiK`, ensure inittab boots `/media/fat/MiSTer`, and reboot.

Like Zaparoo, the fork must suppress the normal menu path as soon as the
alternate launcher is configured. The fork writes a tiny `/tmp/mister_magik_launcher`
script, starts it through `/sbin/agetty` on `tty2`, switches to VT2, releases
Main's input grab, and hides any visible menu OSD. Main must not write
`/sys/module/MiSTer_fb/parameters/mode` or send its generic 8888
`video_fb_enable(1)` while the MagiK launcher is active; Rust owns the
framebuffer format/stride and `SET_FBUF` route. The one intentional pre-UI frame
is Rust-owned black from `mister-magik-fb early-black`, run after Main's
`video_init()` and before the child launcher is spawned. This path depends on
the launcher-specific `[Menu] direct_video=0` / `video_mode=8` settings.

**Game launch:** Slint spawns MiSTer if needed, writes `load_core <path.mra>` to
`/dev/MiSTer_cmd`, then exits when the arcade core is detected. On launch failure,
MiSTer is stopped again so Slint regains display + input. Still **no external
`rbf_load`** from Slint.

**Why not keep MiSTer running (F9 / Zaparoo coexist)?** Stock MiSTer holds
`EVIOCGRAB` on physical pads and composites the FPGA menu OSD unless it processes
F9 internally (`video_fb_enable` → `input_switch(0)`). Injected F9 from uinput is
unreliable; coexistence gave stock menu on HDMI with no Slint input.

**Do not SIGSTOP MiSTer for the launcher.** A stopped MiSTer keeps **evdev grabs**
(joystick dead) and the **FPGA menu OSD** composited on HDMI.

**Why not `main=` on the Slint binary?** `user_io_init` calls
`app_restart(main=…)` *before* `video_init()` (HDMI timing, I2C). Result: Slint
renders to `/dev/fb0` but the TV reports no signal. `main=MiSTer_MagiK` is
different because MiSTer_MagiK is a full Main_MiSTer fork and runs `video_init()`
itself.

**Dev:** `kill -9 $(pidof MiSTer)` then `mister-magik-fb ui launcher …` for a
manual run matching production boot.

Cross-built binary at `/media/fat/mister-magik/mister-magik-fb`. Subcommands:
`read`, `fb`, `ui` (default scene `launcher`), `input`, `scenes`.

Launcher: D-pad nav, A to open controller test or launch arcade games. Launch spawns
MiSTer briefly, sends `load_core` via fifo, shows loading overlay until core runs.

**HDMI recovery:** If launch leaves a black screen, `scripts/mister reboot`
(MiSTer still owns recovery when running; if wedged, reboot always works).

### Main fork experiment — Main as parent of Slint

`main-mister/` is a buildable Main_MiSTer fork for coexistence experiments. The
device deploys it as `/media/fat/MiSTer_MagiK` and selects it through
`[MiSTer] main=MiSTer_MagiK`, while `magik-gui/` stays the separate Slint binary
project.
Use `scripts/deploy-main-mister-experiment.sh` to build/deploy both.

Current intended flow (2026-06-06): `/etc/inittab` starts stock
`/media/fat/MiSTer`; `MiSTer.ini main=MiSTer_MagiK` hands off to the fork; the
fork initializes the menu core, then starts
`/media/fat/mister-magik/mister-magik-fb ui launcher 0` as a child through the
Zaparoo-style `agetty`/tty2 handoff. The command
`mister_magik_launch <absolute .mgl/.mra path>` on `/dev/MiSTer_cmd` shuts down
the Slint child and launches through Main's existing path.

Fork baseline policy: pin `main-mister/` to upstream Main_MiSTer release commits
named `Release YYYYMMDD.` that update `releases/MiSTer_YYYYMMDD`, not arbitrary
development commits. Current baseline is `Release 20260603`
(`c73802332ff9c73659410084b6319ccd29f0b3aa`).

Important gotchas:

- Disable the old `/etc/inittab` `mister-magik/boot.sh` handoff. Production
  inittab must boot `/media/fat/MiSTer`; `deploy-main-mister-experiment.sh`
  installs/repairs `[MiSTer] main=MiSTer_MagiK`.
- `deploy-main-mister-experiment.sh` and `install-slint-boot.sh` copy
  `/media/fat/MiSTer.ini` to `/media/fat/MiSTer.ini.bak` if needed, then edit
  `MiSTer.ini` only through the Rust comment-preserving mutator in
  `scripts/mister`. Do not use AWK/sed/ad hoc shell to edit `MiSTer.ini`.
- Main clean rebuilds matter. If `main-mister/bin/MiSTer` is stale, new
  `support/mister_magik/*.cpp` files may not be reflected in the deployed binary.

### TODO

- Derive `xoff/yoff`/geometry from the **live** video mode (`rust-livemode`).
- In-game settings: keep stock OSD vs fork Main_MiSTer (see §11). Fork surface
  checklist: [`history/2026-6-3/zaparoo-fork-surface.md`](history/2026-6-3/zaparoo-fork-surface.md).
- ~~Controller input~~ — basic js API wired; polish mapping / hot-plug.

---

## 8. Lessons learned / gotchas

- **SSH:** use `scripts/mister`, the Rust wrapper. Raw `ssh`/`scp` hit "Too many
  authentication failures" (client offers every agent key) and the documented
  MiSTer pubkey-auth hang. `expect` works but is brittle for long/streaming
  commands. **Prefer `scripts/mister` for everything.**
- **`/dev/fb0` ≠ HDMI** at the menu (§3). Don't trust "it's in the framebuffer"
  as "it's on screen." Always confirm on the actual HDMI output / by knowing the
  fb-enable state.
- **fbcon clobbers `/dev/fb0` → black screen.** The kernel framebuffer console
  (`vtcon1`) can clear the buffer. The demo UI uses `animation-tick()` for
  continuous repaints; a static UI may need `KD_GRAPHICS` / unbind fbcon (§11).
- **`ui` / `fb` call `KD_GRAPHICS` on `/dev/tty0`** (`magik-gui/src/vt.rs`) so fbcon
  stops drawing the blinking block cursor over the title (confirmed in framebuffer
  PNGs). Restores `KD_TEXT` on exit. If the ioctl fails, we log and continue.
- **busybox has no `pkill`.** Use `kill -9 $(pidof mister-magik-fb)` to stop the app.
- **libinput quirks DB missing** → `libinput error: ... device quirks` warnings.
  Rendering is fine; if/when we add input, bundle the quirks DB or point
  libinput at one.
- **Fonts:** MiSTer has none. The Rust build embeds Press Start 2P (`magik-gui/ui/fonts/`).
- **Framebuffer byte order is BGRX** (`rgba 8/16,8/8,8/0`). `scripts/mister
  raw-to-png` swaps B/R; keep that in mind for any direct fb work.
- **Don't use `main=mister-magik-fb`.** Skips `video_init()` → no HDMI signal
  (§7). Use `main=MiSTer_MagiK`; the fork is Main and can initialize video.
- **Don't external `rbf_load` from Slint.** `core_reset` + failed `socfpga_load`
  leaves the FPGA without valid scan-out → TV "no signal". Use fifo `load_core`
  on the running MiSTer process (§7).
- **Don't SIGSTOP MiSTer for the launcher.** Stopped MiSTer keeps evdev
  grabs (no joystick) and FPGA menu OSD over Slint on HDMI.
- **Don't leave the menu paused.** If you `kill -STOP $(pidof MiSTer)` for an
  experiment, always `kill -CONT` it afterwards (or reboot).
- **Slow SSH after reboot = DHCP, not sshd.** `sshd` listens ~kernel 9 s, but
  `eth0` is managed by `dhcpcd` (it's *not* in `/etc/network/interfaces`), and the
  default path wasted ~17 s: DHCP solicit timeout → IPv4LL (169.254.x) fallback →
  ARP DAD probing before the real lease. A static IP in `/etc/dhcpcd.conf`
  (`noarp`, `noipv4ll`) skips all of it. Rootfs is read-only — remount rw to edit.
  See §10.
- **FPGA SPI from our process** routes `/dev/fb0` scan-out during Slint UI
  (`magik-gui/src/fpga.rs`). Production stops MiSTer first so we own the bus; no
  SIGSTOP dance needed. Historical dev spike (§9.5) used SIGSTOP for standalone tests.
- **Don't blind-sleep on reboot.** The device reboots fast (~35s to userspace,
  drops off the network in ~3s). Use `scripts/mister reboot-wait` (or `wait`),
  which detects the down→up transition and returns the instant `pidof MiSTer`
  answers, instead of a fixed `sleep`. Polling port 22 alone is not enough —
  it confirm-runs a command so we don't act before the rootfs is ready.
- **HDMI audio from Slint uses `/dev/MrAudio`, not ALSA.** Kernel source
  `sound/drivers/MiSTer-audio-spi.c` creates the char device. Normal writes copy
  32-bit-aligned chunks into the MiSTer audio buffer and send an SPI
  `{addr,len,ptr,reserved}` update; reads return status text
  (`rptr/wptr/len/comp`). A standalone `audio-tone` probe produced audible HDMI
  tone with MiSTer stopped, confirming Slint-owned boot can feed audio.
- **Library scan must not count core helper payloads as games.** Some systems
  store launchable-looking support files under `/media/fat/games/<system>/`,
  e.g. `boot.rom`, `boot3.vhd`, `mister-boot.*`, `riscos.rom`, `kanji.rom`,
  `uni-bioscd.rom`, and `Super Game Boy.sfc`. Filter these before writing or
  grouping catalog discoveries; otherwise empty systems show as having 1-2 games.
  Also treat raw `.rbf` core binaries and menu-level `.mgl` launchers in
  `_Computer` / `_Console` / `_Other` / `_Utility` as helpers, while preserving
  DOS game `.mgl` files whose payload is under `media/...`.
- **AmigaVision is a launcher HDF, not a raw compressed-game set.** The MiSTer
  entry is `_Computer/Amiga.mgl` (`setname=Amiga`) and requires the extracted
  `games/Amiga/AmigaVision.hdf`, ROM/config files, listings, and optional
  `games/Amiga/shared` directory from the AmigaVision MiSTer archive. Individual
  game launches work by writing a title from `games/Amiga/listings/games.txt` to
  `games/Amiga/shared/ags_boot` before loading `_Computer/Amiga.mgl`; AmigaVision's
  startup sequence consumes that file and runs the matching AGS launch script.
  Parse `games/Amiga/listings/*.txt` lossily because the extracted lists may
  contain non-UTF-8 title bytes.
  Do not treat the top-level `AmigaVision*.7z` itself as a playable launch
  target. The upstream MiSTer package removes `games/Amiga/Visuals`, so screenshots
  for MagiK need a separate source such as upstream `data/img/*.iff` conversion or
  HDF/AGS extraction if present.
- **Perf runs can be contaminated by a 30fps cadence.** We observed benchmark
  scenes sometimes starting in a bad 30fps/vsync phase after repeated Slint
  restarts or immediately after deploy/reboot, then recovering later. For
  performance comparisons, put the MiSTer in a clean state, kill any existing
  `mister-magik-fb`, wait/settle before starting the scene (5s worked in tests),
  and distrust short runs whose first seconds show `fps ~ 30` unless that is the
  thing being measured. Reboot and rerun before concluding a copy/render change
  regressed performance.
- **Visual benchmarks must not leave the fork parent running.** In the
  Main-as-parent experiment, `/media/fat/MiSTer_MagiK` is the fork parent,
  and that parent can keep the stock FPGA OSD/menu compositor
  alive over standalone `mister-magik-fb ui <scene> ...` benchmark runs. Before
  any visual benchmark that is not intentionally testing coexistence, stop all
  three possible owners: `mister-magik-fb`, `MiSTer_MagiK`, and `MiSTer`; then
  start the benchmark scene after the normal settle delay. If the original OSD is
  visible over the benchmark, the run is invalid even if the framebuffer PNG
  looks correct.
- **Use MiSTer preset IDs for standard HDMI sweep modes.** Shorthand calculated
  modes such as `1280,720,60` ask MiSTer to synthesize CVT-RB timings; on the
  current TV that made stock MiSTer, games, and the Slint framebuffer jump
  badly. `720p` should use preset `0`, `1080p` preset `8`, and `640x480` preset
  `6`. The documented 1440p preset `14` is a pixel-repetition mode
  (`2560x1440@60` with internal `1280x1440` timing) and is display-dependent:
  stock MiSTer was glitchy on the current TV with both preset `14` and calculated
  `2560,1440,60`. For “whatever this display actually supports”, comment
  `[Menu] video_mode` and let MiSTer use EDID/native detection; on the current TV
  that selected stable `1920x1080`.
- **CRT/direct-video menu timings ignore `[Menu] video_mode`.** In Main_MiSTer,
  `direct_video=1` switches the menu path to `tvmodes[]`: `menu_pal=0/1` chooses
  NTSC/PAL (`640x240` or `640x288`), and `forced_scandoubler=1` selects the 31 kHz
  variant (`640x480` or `640x576`). `direct_video=2` is only auto-detect for known
  HDMI DACs and resolves back to normal HDMI when the attached display is not a
  known DAC. Use `scripts/mister-video-mode-test.sh crt-list` and `crt-smoke`
  for opt-in smoke tests, and always restore the persistent INI backup after any
  direct-video run. On 2026-06-07, `crt-smoke direct-auto stock` was stable on
  the HDMI TV and resolved to normal `1920x1080`; `crt-smoke ntsc31 stock`
  (`direct_video=1`, `forced_scandoubler=1`, `menu_pal=0`) was also stable, with
  Linux fb `640x480` and the TV reporting `529x480p` with TV-managed aspect
  ratio.
- **Boot flicker analytics found Main OSD still updates after Slint handoff.**
  The `2026-06-07` analytics run recorded repeated post-handoff
  `main-osd OsdUpdate dirty_lines=3 n=19 is_menu=1 osdset=0x70000` events while
  Slint was already rendering. It also found the old fixed-mode reassert loop
  and right-edge `/dev/fb0` changes. See
  `history/2026-6-5/boot-flicker-analytics.md`.
- **Boot black screen was caused by routing an unpainted framebuffer.**
  Whole-boot analytics showed Main routing HDMI to `/dev/fb0` while the buffer
  was still black, then Slint waiting on cached arcade catalog load before its
  first render. The current handoff makes Rust clear a black frame and route
  `/dev/fb0` itself before constructing the full UI. Static visible before
  `MiSTer_MagiK main_start` is outside the fork lifetime, likely stock
  `/media/fat/MiSTer` before `main=` reexec. See
  `history/2026-6-7/whole-boot-visual-analytics.md`.
- **Do not mark the launcher runtime-active before `video_init()`.** That was
  tried while diagnosing boot static and broke Rust `SET_FBUF` support reads
  (`support_flag=0` / zeroed UIO reads). Early suppression before the child
  starts must be OSD-only; framebuffer ownership begins with the Rust
  `early-black` helper after Main has initialized video. See
  `history/2026-6-12/slint-owned-magik-framebuffer.md`.

---

## 9. Performance (rendering)

See `history/2026-5-2/framebuffer-experiments.md` for the full experiment log.

### 9.5 FPGA direct-access spike — fork-free `/dev/fb0` → HDMI (✅ proven)

Goal: prove we can route `/dev/fb0` to HDMI **from our own process**, as the
foundation for the Rust frontend and (eventually) a `main=` boot binary.
We validated the FPGA layer with a throwaway **Python** port of MiSTer's SPI code
(`history/2026-5-2/framebuffer-experiments.md` — the Python `scripts/fpga_*`
probes were removed; the real impl is Rust). Method: boot to the
stock menu, `kill -STOP $(pidof MiSTer)` to own the SPI bus (HDMI stays live —
scan-out is FPGA-driven), poke registers, `kill -CONT` after.

**Proven facts (these de-risk the Rust port):**

- **FPGA registers = `mmap(/dev/mem)` + volatile u32.** The HPS↔FPGA "SPI" is the
  FPGA-manager GPO/GPI pair at `0xFF706000 + 0x10` (out) / `+0x14` (in). Map that
  page, done. Addresses confirmed live.
- **The bit-bang handshake works from our process.** `fpga_spi(word)`: set data +
  strobe (`GPO bit17`), wait `GPI bit17` (ACK) high, drop strobe, wait ACK low.
  Instrumented: idle `GPI=0x00040000` (io_ver=1); strobe-high → `0x00060001`
  (ACK + data present); strobe-low → `0x00040000`. Real handshake, not a stale read.
- **Chip-select framing matters.** If MiSTer is SIGSTOPped mid-transaction, `IO_EN`
  (`GPO bit20`) may be high; drop then raise it (`DisableIO`→`EnableIO`) so the
  command word isn't mis-parsed as a parameter.
- **`video_fb_enable(1,0)` routes `/dev/fb0` to HDMI — confirmed visually.**
- **`/dev/fb0` phys addr = `0x22001000`** (`smem_start`) = `FB_ADDR(0x22000000)
  + 4096` (the `n?0:4096` params page). Matches `video.cpp`.
- The 10-word SET_FBUF sequence (after `spi_w(UIO_SET_FBUF=0x2F)`): `fmt`
  (`FB_EN|FB_FMT_RxB|FB_FMT_8888 = 0x8016`), addr-lo, addr-hi, width, height,
  scaled L, scaled R, scaled T, scaled B, stride.

**Historical direct_video=1 notes (why the spike showed colour columns):**

- In the old direct-video spike, `video_fb_enable` positioned the fb via
  `xoff = v_cur.item[4] - FB_DV_LBRD(3)`, `yoff = v_cur.item[8] - FB_DV_UBRD(2)`,
  uses tiny border porches (`FB_DV_*`), and calls `set_vga_fb()`. Our probe used
  `xoff=yoff=0` (non-direct path) → misaligned columns. The Rust port must reuse
  MiSTer's positioning math (and the live mode `video_mode=8` = 1080p), not
  hardcode 1920×1080.
- **Multi-word SPI *reads* were flaky in Python.** Command-word reads work (got the
  `GET_FB_PAR` CRC `0x6d`, the SET_FBUF "supported" flag `0x1`), but `UIO_GET_VRES`
  / `UIO_GET_FB_PAR` data words all read 0 — a back-to-back read-timing artifact of
  slow Python (or the menu core not implementing them). At native Rust speed, port
  `fpga_spi` faithfully; MiSTer reads these fine.
- VT/graphics-mode context (so fbcon doesn't clobber, §8/§11) is still required —
  that's the genuinely fiddly part, not the SPI.

### 9.6 Native Rust port — clean fork-free image (✅ done)

The spike is now reproduced in Rust (`magik-gui/` crate) and renders a **clean,
full-screen image from our own binary**:

- `magik-gui/src/fpga.rs` ports the SPI layer (`mmap` GPO/GPI, EnableIO/DisableIO,
  `fpga_spi`) and `video_fb_enable(1,n)` (the SET_FBUF sequence). **Native-speed
  multi-word reads work** (GET_VRES/GET_FB_PAR return stable data; ACK-high ==
  ACK-low), unlike the slow Python which read 0s.
- `magik-gui/src/fb.rs` mmaps `/dev/fb0` (1920x1080 xRGB8888) for direct pixel writes.
- `mister-magik-fb fb [xoff] [yoff]` paints a 4-quadrant + border + cross-hair
  test pattern and routes buffer 0. `read` dumps the live mode/fb params.
- Live values read from the stock menu: `GET_FB_PAR` → `fb_w=1920 fb_h=1080
  fb_fmt=0x00d6 fb_en=1`; `GET_VRES` → `width=529 height=240 pixrep=2` (the
  direct_video native scan-out, not the fb size).

**Geometry lesson (why the columns, then the offset):**

- Columns = wrong horizontal *span*. The scaled right coord must be
  `xoff + v_cur.item[1] - 1`; with `item[1]=1920` that's a 1920-wide span in the
  direct_video raster (which is ~2147 wide). Sending `right=1919` from `left=0`
  is still a 1920 span, but earlier Python also mis-sent params; the Rust port
  with the correct sequence does **not** shear.
- Offset = wrong `xoff/yoff`. The `direct_video` formula is
  `xoff=item[4]-FB_DV_LBRD(3)`, `yoff=item[8]-FB_DV_UBRD(2)`. Using the *original*
  mode-8 porches (`148/36`) put us ~145px too far right. The **running menu mode**
  already has the tiny border porches (`item[4]=3, item[8]=2`), so the correct
  values are **`xoff=yoff=0`**. Confirmed clean on HDMI at `0,0`.

**Resolution / CRT generality (answer to "will this cope with lower res / CRT?"):**
The production HDMI UI path is intentionally optimized for the known stable
1080p boot configuration: set `/dev/fb0` to 960×540, render Slint 1:1, and route
that source through the FPGA scaler to 1920×1080. CRT/`direct_video` and other
display modes remain opt-in smoke tests; do not assume the 960×540 UI path is
already generalized to every output timing.

### 9.7 Slint software renderer @ locked 60fps — smooth + tear-free (✅ done)

`mister-magik-fb ui [secs]` runs Slint's **software renderer** for a 960×540
framebuffer (no X/Wayland) at a **rock-steady 60fps, smooth and tear-free**
(confirmed on HDMI). The FPGA scales the 960×540 source to 1080p, so the ARM no
longer performs the full-framebuffer 2× expansion. Current demo smoke budget
(2026-06-08):

```
render ~0.9ms (cached RAM)  +  vsync-wait ~15.0ms  +  dirty copy ~0.7ms  ≈ 16.6ms
```

**Architecture (the bits that matter):**

- `MisterPlatform` implements Slint's `Platform` trait: one `MinimalSoftwareWindow`
  (`RepaintBufferType::ReusedBuffer`), time from a monotonic `Instant`. We drive
  the loop ourselves (no `run_event_loop`), pacing each frame on
  `FBIO_WAITFORVSYNC`.
- Render into a **cached** `Vec<Pixel>` (fast, ~0.9ms for the demo — Slint only
  redraws the dirty region). `render()` returns a `PhysicalRegion`; broad
  updates copy rows and narrow updates copy the reported rectangle.
- Do **not** render Slint directly into the live `/dev/fb0` buffer for production
  UI. The 2026-06-08 `direct-fb` trial removed the copy and improved CPU/frame
  time, but visible HDMI flicker remained even with `vsync-first` and post-vsync
  delay experiments. The 2026-06-09 sidecar-module attempt exposed non-live
  write-combined backbuffers, but real Slint UI still showed visual glitches and
  worse CPU cost than the cached path. Keep the production path cached and
  optimize there; see
  `history/2026-6-9/direct-framebuffer-sidecar-retrospective.md`.
- Before opening `/dev/fb0`, the UI path writes the MiSTer framebuffer mode as
  `8888 1 960 540 3840`, opens that small write-combined buffer, and routes it
  once via `fb_enable(0, 960, 540, Mode { hact: 1920, vact: 1080, hbp: 3, vbp:
  2 }, …)`.

**THE key hardware finding — write-combining vs uncached:**

- `/dev/fb0`'s driver mmap is **write-combining (~700 MB/s)**.
- mmapping the *same* physical buffers via `/dev/mem` (as MiSTer's `shmem_map`
  does, `O_RDWR|O_SYNC`) is **uncached device memory (~105 MB/s)** — ~7× slower.
- Consequence: **true FPGA page-flipping is a dead end here.** It's genuinely
  tear-free and the flip itself is ~12µs, BUT the back buffers (1/2) are only
  reachable via `/dev/mem`, so writing them is too slow — a 620-row update took
  **45ms** (→ 20fps). Rendering *directly* into a `/dev/mem` buffer
  (`SwappedBuffers`) was ~15–17ms and unstable (dropped to 30fps under load).
  `/dev/fb0` only ever exposes **one** buffer (`virtual_size` = 1920×1080), so we
  can't get a second *write-combining* buffer to flip between.
- The winning compromise is now a single small write-combined buffer plus FPGA
  output scaling. In the `FPSCALE-CLEANUP-UI-SMOKE-20260608` demo run, normal UI
  copy time was ~0.7ms at 60fps with `fb_size=960x540`, `render_size=960x540`,
  and `fb_scale=1`.
- Dirty framebuffer copies use rectangle copies for narrow/medium dirty boxes
  and full-row copies for broad boxes. `MISTER_DIRTY_RECT_BROAD_PCT` defaults to
  `85` after the `CACHED-RECT85-VIDEO-20260608` sweep: it kept demo/console
  close to baseline while improving `video_playback` versus the old 75% cutoff.

**Slint build notes (cross-compile, no system fonts):**

- Build Slint with `default-features=false`, features
  `["compat-1-2","renderer-software","unsafe-single-threaded","libm"]` — **no
  `std`** (the `std` feature pulls system font loading → `fontconfig`, which the
  bare cross container and the MiSTer don't have). Our own crate still uses `std`.
- `build.rs` uses `EmbedResourcesKind::EmbedForSoftwareRenderer` so glyphs/images
  are baked into the binary. With no system fonts, the embedder needs a font:
  we bundle `magik-gui/ui/fonts/PressStart2P-Regular.ttf` (SIL OFL) and set
  `default-font-family: "Press Start 2P"` on the Windows.
- Slint's deps need **rustc ≥ 1.90**; the toolchain is pinned to `stable`
  (1.96 at time of writing) in `magik-gui/rust-toolchain.toml`, with a matching
  `stable-x86_64-unknown-linux-gnu` installed (`--force-non-host`) for the
  emulated cross container.
- **Two release profiles** (`magik-gui/BUILD.md`): `release` (fast daily, thin LTO)
  and `release-device` (fat LTO + Cortex-A9, ship to MiSTer). `opt-level = 3` on both.

**Known follow-ups for the real frontend:** generalize the small-framebuffer
route from the known-good 1080p HDMI mode to other output timings, and add an
explicit HDMI capture path if visual correctness needs to be proven beyond the
960×540 `/dev/fb0` PNG plus route logs.

---

## 10. Current device state & recovery

- **Boot:** `/etc/inittab` → `/media/fat/MiSTer`; `[MiSTer]
  main=MiSTer_MagiK` hands off to the Main fork; the fork starts
  `/media/fat/mister-magik/mister-magik-fb ui launcher 0`.
- `MiSTer.ini` must **not** set `main=mister-magik-fb`; Slint is not Main and
  cannot initialize HDMI before `main=` re-exec.
- `[MiSTer] main=MiSTer_MagiK`, `[MiSTer] direct_video=0`, `[Menu]
  direct_video=0`, and `[Menu] video_mode=8` for the launcher path. Normal
  arcade direct-video belongs in `[arcade] direct_video=1`. Rotated arcade games
  must keep `[arcade_vertical] direct_video=0` and `video_mode=8` so MiSTer
  outputs rotated 1080p instead of asking the external scaler to rotate.
- **Static IP `192.168.1.117` (no DHCP).** See §8 for dhcpcd.conf details.
- Fork binary at `/media/fat/MiSTer_MagiK`; Slint child at
  `/media/fat/mister-magik/mister-magik-fb`.
- **Recovery to stock menu:** `scripts/restore-stock-boot.sh` copies
  `/media/fat/MiSTer.ini.bak` back to `/media/fat/MiSTer.ini`, ensures inittab
  boots `/media/fat/MiSTer`, and reboots. Works over SSH even with no HDMI.

---

## 11. Open questions / follow-ups

- **Cleaner fix than the always-on animation** in the demo UI (full-width dirty
  rows). A real UI with localised motion copies less. fbcon may still clobber a
  static frame — investigate `KD_GRAPHICS` / unbind fbcon (see §8).
- Return-to-launcher after game reset (without full reboot).
- libinput quirks DB is missing — js API works; revisit for hot-plug polish.

---

## 12. Rust ARM toolchain

Cross-compile toolchain is **proven end-to-end**: a binary built on the
Apple-Silicon host runs on the MiSTer (`arch=arm, os=linux, glibc 2.31`).
On Apple Silicon, `magik-gui/build-arm.sh` uses the Apple Virtualization
Framework container backend by default and mirrors the finished binary into the
normal `magik-gui/target/...` path. CI/Linux still use cross-rs via Docker.

**Build & deploy:**

```bash
scripts/deploy-rust.sh                   # release-device (full MiSTer build)
scripts/deploy-rust.sh --video           # includes minimal static FFmpeg + video asset
# or manually:
magik-gui/build-arm.sh --device
magik-gui/build-arm.sh --device --video
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/mister \
  put magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb \
  /media/fat/mister-magik/mister-magik-fb
```

**Host dev checks:** use `scripts/dev-rust fmt`, `scripts/dev-rust test`, and
`scripts/dev-rust check` for routine local validation. Before making any commit
that touches Rust code or CI, also run strict Clippy for both Rust crates:
`cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features
-- -D warnings` and `cargo clippy --manifest-path tools/mister/Cargo.toml
--all-targets -- -D warnings`. These checks run the host-testable Rust library
with `--no-default-features`, so pure logic tests do not compile the Slint UI
binary or macOS AppKit code. Use `scripts/dev-rust build-ui` or
`magik-gui/build-arm.sh` when you need the ARM Slint binary.

Every `magik-gui/build-arm.sh` run prints the binary size and appends a local,
gitignored row to `build/binary-size.tsv` keyed by profile + features. Keep the
formal benchmark/size history in `history/toolchain-bench/results.tsv` via
`scripts/bench-toolchain.sh`.

GitHub Actions CI lives in `.github/workflows/rust-arm.yml`. It builds
`--device` and `--device --video`, uploads the ARM
binaries and size TSVs, and runs `magik-gui/scripts/check-arm-shared-libs.sh` so
video builds fail if FFmpeg becomes a runtime `libav*` dependency.

Video builds use a project-local **minimal static FFmpeg 8.1.x** built by
`magik-gui/scripts/build-minimal-ffmpeg.sh` under `magik-gui/target/ffmpeg-minimal/armv7`.
It enables only H.264 decode/parse, `pcm_s16le`, MOV/MP4 demuxing, file
protocol, and `avcodec`/`avformat`/`avutil`/`swscale`; no system `libav*`
runtime is required. `video_playback` defaults to
`/media/fat/mister-magik/mslug3.mov` (H.264 baseline + 48 kHz stereo signed PCM)
and writes PCM packets directly to `/dev/MrAudio` with video as the master clock.
Video CPU benchmark notes from 2026-06-06: passing a
`SharedPixelBuffer<Rgb8Pixel>` from the decode worker avoids an extra
RGB byte-copy on the UI thread and reduced `video_playback` CPU mean from
`75%` to `64-67%` on the Cortex-A9. RGBA output was worse (`85%` mean), and a
naive fixed `yuv420p`→RGB converter was also worse (`70%`) than swscale. FFmpeg
logs `No accelerated colorspace conversion found from yuv420p to rgb24`, so the
next likely win is a genuinely optimized color-conversion path or cheaper H.264
encode settings, not another generic build-flag tweak.

For binary-size diagnosis, build an unstripped profile binary and group symbols:

```bash
magik-gui/build-arm.sh --profile
magik-gui/scripts/analyze-binary-size.sh
```

See **`magik-gui/BUILD.md`** for profile table, size tracking, FFmpeg notes, and bench
mapping (A0 ≈ `release`, A3 ≈ `release-device`).

**One-time host setup (done):**

```bash
rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host
rustup target add armv7-unknown-linux-gnueabihf --toolchain stable-aarch64-unknown-linux-gnu
container system start
container builder start --cpus 3 --memory 5g
```

**Apple-Silicon gotchas (all handled by `build-arm.sh` + config, but know them):**

- **Backend selection:** local Apple-Silicon builds default to
  `MISTER_ARM_BUILD_BACKEND=apple-container`. Set
  `MISTER_ARM_BUILD_BACKEND=cross` only when intentionally comparing against the
  CI/Linux Docker path.
- **glibc match:** `cross` 0.2.5 images are Ubuntu 20.04 = **glibc 2.31**, which is
  what the MiSTer runs. So the default image Just Works — no musl, no static.
- **`--force-non-host`:** the Apple container mounts a Linux/aarch64 Rust
  toolchain into the Linux VM, so `stable-aarch64-unknown-linux-gnu` must be
  installed on the Mac even though it cannot run directly on macOS. rustup blocks
  it without `--force-non-host`.
- **sccache wrapper:** this repo must not use `sccache`. The root
  `.cargo/config.toml` and `magik-gui/.cargo/config.toml` set
  `rustc-wrapper=""`, and Rust workflow scripts export `RUSTC_WRAPPER=""` so a
  global `~/.cargo/config.toml` or shell environment cannot route builds through
  a missing/host-specific wrapper.
- **toolchain pin:** `magik-gui/rust-toolchain.toml` pins `stable` + the armv7 target.

**Crate layout:**
- `magik-gui/src/fpga.rs` — SPI layer + `video_fb_enable` port (§9.6), UIO/FB constants.
- `magik-gui/src/fb.rs` — `/dev/fb0` mmap wrapper for direct pixel writes.
- `magik-gui/src/main.rs` — `read` | `fb` | `ui` subcommands.

See §7 for roadmap.
