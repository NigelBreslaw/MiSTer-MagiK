# Main vsync framebuffer flip experiment - 2026-07-08

## Question

Can Main flip between two HPS framebuffer addresses during vblank without visible tearing?

## Summary

Yes, Main can drive the FPGA framebuffer route through the existing Menu core
`UIO_WAIT_VSYNC` and `UIO_SET_FBUF` path. A Main-owned diagnostic filled two
hidden RGB565 buffers and alternated the scanout route between them.

The first frame-counter version flipped by polling `UIO_GET_FR_CNT`. It proved
address switching worked, but HDMI capture showed horizontal split-frame tearing.

The successful version used `UIO_WAIT_VSYNC` immediately before each
`UIO_SET_FBUF`. The important protocol detail is that `UIO_WAIT_VSYNC` is a bus
wait, not a support query: it holds the UIO transaction until HDMI vsync and may
return zero. Treating zero as "unsupported" caused an earlier false negative.

## Evidence

Main-side timing from the instrumented run showed 600 completed flips in about
10 seconds. After the first wait, each wait-plus-route iteration landed on a
roughly 16-17ms cadence:

```text
wait_done elapsed_ms=6-7
routed buffer=1/2
next route roughly 16-17ms later
```

HDMI capture artifacts:

- `build/hdmi-capture/main-pattern-flip-framecounter-20260708.mp4` - frame
  counter version, visibly tears.
- `build/hdmi-capture/main-pattern-flip-wait-vsync-instrumented-20260708.mp4`
  - corrected wait-vsync version.
- `build/hdmi-capture/main-pattern-flip-wait-vsync-instrumented-strip.png`
  - overview strip.
- `build/hdmi-capture/main-pattern-flip-wait-vsync-instrumented-active.png`
  - dense active contact sheet.

## Result

Option A remains viable. The next real prototype should not have Rust race
scanout directly. Instead:

1. Rust renders into a hidden buffer.
2. Rust asks Main to present that buffer.
3. Main waits for HDMI vsync with `UIO_WAIT_VSYNC`.
4. Main commits the new route with `UIO_SET_FBUF`.

This gives Main the vblank commit point while keeping Rust responsible for UI
rendering.

## Cleanup

The device was restored to one `MiSTer_MagiK` process and one
`mister-magik-fb ui launcher 0` process. No persistent launcher env, fault
arming file, rebuild marker, or volatile pattern trigger remained.
