# Slint-Owned MagiK Framebuffer Handoff

Date: 2026-06-12

## Problem

Occasionally after a MiSTer reset, HDMI showed a bad launcher state: the screen
looked like 1080p, but the Slint UI was 960x540 and repeated vertically so only
the top half was useful. During boot we also saw a short static/checker/noise
frame before Slint appeared.

The review finding was that framebuffer ownership was split:

- Main_MiSTer wrote `/sys/module/MiSTer_fb/parameters/mode` with a hard-coded
  8888 stride path.
- Main also sent generic `video_fb_enable(1)` routes while the MagiK launcher
  was starting.
- Rust/Slint was separately choosing 565 or 8888, opening `/dev/fb0`, and
  routing buffer 0.

That made mixed states possible, especially `8888 1 960 540 1920`, and made a
future return to 8888 more fragile than it needed to be.

## Final Design

The stock boot model stays the same:

```text
/etc/inittab -> /media/fat/MiSTer
[MiSTer] main=MiSTer_MagiK
MiSTer_MagiK -> /media/fat/mister-magik/mister-magik-fb ui launcher 0
```

Only the MagiK framebuffer ownership changed:

1. Stock MiSTer still reexecs `MiSTer_MagiK` through the normal `main=` hook.
2. Main_MiSTer initializes video normally.
3. If the MagiK launcher binary exists, Main runs
   `mister-magik-fb early-black` once after `video_init()`.
4. The Rust helper writes the selected framebuffer mode from
   `MISTER_FB_FORMAT`, opens `/dev/fb0`, clears it to black, and sends
   `SET_FBUF` with the matching format and stride.
5. Main starts the normal Slint child through the existing `agetty`/tty2
   handoff, but no longer calls `video_fb_enable(1)` for the launcher.
6. The full Slint UI repeats the Rust-owned mode/clear/route step, then draws
   the launcher.

Main still owns fallback paths. Explicit exit to the stock menu and launcher
crash/give-up paths can re-enable the normal Main menu behavior after marking
the launcher inactive.

## Important Trap

Do not set `mister_magik_launcher_active()` early just to suppress the static.
That was tested and broke Rust framebuffer routing: `SET_FBUF` reported
`support_flag=0` and UIO reads came back zero. The safe split is:

- early boot: OSD suppression intent only;
- after `video_init()`: Rust `early-black` owns the first MagiK frame;
- after child spawn: launcher runtime-active suppresses Main fb mode/route
  writes.

## Implementation Map

- `main-mister/video.cpp`
  - suppresses `fb_write_module_params()` while the launcher is active;
  - suppresses generic `video_fb_enable()` while the launcher is active;
  - emits analytics for both suppressed paths.
- `main-mister/support/mister_magik/alt_launcher.cpp`
  - tracks early OSD suppression intent separately from launcher runtime-active;
  - runs the Rust `early-black` helper;
  - defers launcher framebuffer routing to Slint and only releases input.
- `main-mister/osd.cpp`
  - uses the early OSD suppression predicate, not only runtime-active.
- `main-mister/user_io.cpp`
  - calls the Rust early-black helper immediately after `video_init()` for the
    menu core path.
- `magik-gui/src/main.rs`
  - adds the `early-black` command.
- `magik-gui/src/ui_runner.rs`
  - clears black before the normal Slint route.
- `magik-gui/src/fb_format.rs` and `magik-gui/src/fb.rs`
  - preserve 565 vs 8888 and the live stride when restoring mode lines.

## Validation

Host checks passed:

```text
scripts/dev-rust fmt
scripts/dev-rust test
scripts/dev-rust check
scripts/dev-rust host-tools
main-mister/build-docker.sh clean
main-mister/build-docker.sh
git diff --check
```

Device validation after deploy:

```text
fb_mode=565 1 960 540 1920
bpp=16
virtual_size=960,540
stride=1920
early_black_route_completed format=565 w=960 h=540 scan=1920x1080 support_flag=1
rust_framebuffer_route_completed format=565 w=960 h=540 scan=1920x1080 support_flag=1
```

The user confirmed the visible result after the early-black fix: no static and
the boot path goes directly to the Slint UI.

## Remaining Scope

Some visual output before `MiSTer_MagiK` starts can still exist because stock
`/media/fat/MiSTer` runs first and performs the `main=` reexec. Eliminating that
earlier stock window would be a separate direct-boot/inittab design and is out
of scope for this change.
