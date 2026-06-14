# Launcher Framebuffer Route Reassertion - 2026-06-14

## Context

After the library scanner benchmark, the MiSTer was visibly showing the stock
Menu/static animation while:

- Main reported `launcher_active=true`, `active_vt="tty2"`, and
  `visible_owner="core"`.
- Slint reported the launcher alive at about 60fps.
- `/sys/module/MiSTer_fb/parameters/mode` was the expected
  `565 1 960 540 1920`.

This looked like a possible OSD suppression leak, but the Main code path did not
support that interpretation. `scheduler_co_ui()` skips `HandleUI()` and
`OsdUpdate()` while the MagiK launcher is active, and the OSD entry points call
the MagiK suppression predicate.

## Finding

The visible static was a framebuffer ownership problem, not normal OSD work
continuing to run.

Slint sent the normal RGB565 route at startup:

```text
rust_framebuffer_route_completed format=565 w=960 h=540 scan=1920x1080 support_flag=1
```

Main still reported `visible_owner="core"` because Rust's direct SPI route is
outside Main's `video_fb_enable()` state bookkeeping. Treat that field as
Main-owned route state only; it is not an authoritative FPGA readback once Rust
owns the launcher framebuffer.

A second, related problem made diagnosis confusing: helper commands such as
`fb-format-smoke` can repaint `/dev/fb0`. The launcher was then allowed to
continue in dirty-row mode, so only small Slint updates such as the clock could
be copied back. That left `/dev/fb0` mostly black or stale even though the
launcher process was alive.

## Fix

The launcher now owns the route continuously instead of only during startup:

1. `FramebufferRouteGuard` requests a route reassertion on frame 0 and every 60
   frames by default.
2. Each reassertion sends the current RGB565 `SET_FBUF` route through Rust's
   `Fpga::fb_enable_format()`.
3. A successful reassertion also forces a full cached-frame present back to
   `/dev/fb0`, so dirty-row optimization cannot preserve stale helper output.

The cadence is controlled by `MISTER_FB_ROUTE_REASSERT_FRAMES`. Use `0` only for
diagnostics that intentionally need to disable reassertion.

This is intentionally different from the old boot-flicker experiment that reset
the framebuffer mode and route every 10 frames for the first few seconds. The new
path does not rewrite the framebuffer mode, runs at a lower steady cadence, and
is paired with a full present.

## Validation

Host and device checks:

```text
cargo test --manifest-path magik-gui/Cargo.toml --lib
magik-gui/build-arm.sh --device
scripts/deploy-rust.sh --device
```

After deploy, the event log showed:

```text
launcher_fb_route_reasserted frame=0 support_flag=1
launcher_fb_route_reasserted frame=60 support_flag=1
launcher_fb_route_reasserted frame=120 support_flag=1
```

`scripts/mister status` reported:

```text
fb0: slint_like
slint: scene=launcher screen=home fps=60.x
```

The recovery test intentionally repainted `/dev/fb0` with:

```text
mister-magik-fb fb-format-smoke 565 1 normal
```

After the next route reassertion, `scripts/mister status` again reported
`fb0: slint_like`, and
`build/osd-static-after-fix-recovery/fb0.png` showed the full launcher UI.

## Diagnostic Notes

- Main's `visible_owner` may remain `core` while Slint is visibly correct,
  because Main is not tracking Rust's direct SPI route.
- `UIO_GET_VRES` and `UIO_GET_FB_PAR` can read as zero in this launcher state;
  the `SET_FBUF` support flag and framebuffer classification were more useful
  signals during this investigation.
- If the HDMI output and `/dev/fb0` disagree, capture both the Slint event log
  and `scripts/mister status`; do not assume a live Slint process implies HDMI
  is scanning Slint's buffer.
