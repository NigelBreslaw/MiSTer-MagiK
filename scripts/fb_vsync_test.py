#!/usr/bin/env python3
"""On-device vsync / tearing experiment for the MiSTer framebuffer.

Run THIS on the MiSTer (with the bundled CPython) while watching HDMI. It sweeps
a full-width white bar vertically over a tinted background, updating only the
changed rows each frame (partial redraw, like Slint really does), so both modes
run fast and the ONLY difference is tearing:

  RED background:    NO vsync                       -> expect tearing on the bar.
  GREEN background:  FBIO_WAITFORVSYNC per frame    -> should be clean/smooth.

The RED->GREEN comparison repeats `loops` times after a 3s grey "get ready".

/dev/fb0 is single-buffered on the MiSTer (smem_len == 1 frame, ypanstep == 0),
so this is the "race the beam" test. If GREEN is fully clean we can patch Slint's
linuxfb blit to wait for vsync (Python stays). If GREEN still tears near the top
of the screen, we need the FPGA 3-buffer page-flip (video_fb_enable/UIO_SET_FBUF).

The caller pauses the Slint app (SIGSTOP) first so this owns the framebuffer, and
resumes it (SIGCONT) afterwards.

Usage: fb_vsync_test.py [seconds_per_phase] [loops]   (default 8 2)
"""
import fcntl
import mmap
import os
import struct
import sys
import time

FBIO_WAITFORVSYNC = 0x40044620
FBIOGET_VSCREENINFO = 0x4600
FBIOGET_FSCREENINFO = 0x4602

BAND_H = 140                 # height of the moving bar in pixels
SWEEP_RATE = 1.2             # ~0.83s per top<->bottom sweep (fast = obvious tearing)
WHITE_PX = b"\xff\xff\xff\xff"


def make_bg(b, g, r, size):
    # MiSTer framebuffer byte order is BGRX.
    return bytearray(bytes((b, g, r, 0)) * (size // 4))


def phase(fd, fb, label, tint, vsync, seconds, yres, stride, size):
    bg = make_bg(tint[0], tint[1], tint[2], size)
    work = bytearray(bg)                       # persistent frame; only the band moves
    band = WHITE_PX * (BAND_H * (stride // 4))
    band_bytes = BAND_H * stride
    travel = yres - BAND_H
    last_y = 0
    z = struct.pack("I", 0)
    n = 0
    fb[:] = work                               # one full paint of the background
    t0 = time.perf_counter()
    while True:
        now = time.perf_counter()
        if now - t0 >= seconds:
            break
        pp = ((now - t0) * SWEEP_RATE) % 2.0   # 0..2 ping-pong
        frac = pp if pp < 1.0 else 2.0 - pp
        y = int(frac * travel)
        o = last_y * stride                    # restore old band area from bg
        work[o:o + band_bytes] = bg[o:o + band_bytes]
        o2 = y * stride                        # draw band at new position
        work[o2:o2 + band_bytes] = band
        last_y = y
        if vsync:
            fcntl.ioctl(fd, FBIO_WAITFORVSYNC, z)
        lo = min(o, o2)                         # partial blit: only the dirty rows
        hi = max(o, o2) + band_bytes
        fb[lo:hi] = work[lo:hi]
        n += 1
    dt = time.perf_counter() - t0
    print(f"{label}: {n} frames / {dt:.1f}s = {n / dt:.1f} fps", flush=True)


def main():
    seconds = int(sys.argv[1]) if len(sys.argv) > 1 else 8
    loops = int(sys.argv[2]) if len(sys.argv) > 2 else 2
    fd = os.open("/dev/fb0", os.O_RDWR)
    var = bytearray(160)
    fcntl.ioctl(fd, FBIOGET_VSCREENINFO, var, True)
    xres, yres = struct.unpack_from("<2I", var, 0)
    fix = bytearray(64)
    fcntl.ioctl(fd, FBIOGET_FSCREENINFO, fix, True)
    stride = struct.unpack_from("<I", fix, 44)[0]
    size = stride * yres
    fb = mmap.mmap(fd, size, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    print(f"fb {xres}x{yres} stride={stride} size={size}", flush=True)
    red = (0x18, 0x10, 0x70)
    green = (0x18, 0x70, 0x10)
    grey = (0x20, 0x20, 0x20)
    try:
        fb[:] = make_bg(grey[0], grey[1], grey[2], size)
        time.sleep(3)                          # "get ready" so the viewer can look up
        for i in range(loops):
            phase(fd, fb, f"[{i + 1}] RED   no-vsync", red, False, seconds, yres, stride, size)
            phase(fd, fb, f"[{i + 1}] GREEN vsync   ", green, True, seconds, yres, stride, size)
        fb[:] = make_bg(grey[0], grey[1], grey[2], size)
    finally:
        fb.close()
        os.close(fd)
    print("done", flush=True)


if __name__ == "__main__":
    main()
