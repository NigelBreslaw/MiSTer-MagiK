#!/usr/bin/env python3
"""THROWAWAY on-device probe: replay MiSTer's video_fb_enable(1,0) from our own
process to prove we can route /dev/fb0 to HDMI WITHOUT the Zaparoo binary.

This is a faithful Python port of the exact SPI sequence in
reference/Main_MiSTer/{fpga_io.cpp,spi.cpp,video.cpp}. It is a diagnostic only;
the real implementation will be Rust. Once this works, the register pokes map
1:1 onto Rust (mmap /dev/mem + volatile u32 read/write).

What it does:
  1. Paints an unmistakable test pattern into /dev/fb0 (buffer 0 in DDR).
  2. mmaps the SoCFPGA manager registers (GPO/GPI) via /dev/mem.
  3. Issues video_fb_enable(1, 0): UIO_SET_FBUF + the fb geometry SPI words,
     which tells the FPGA scaler to scan buffer 0 out to HDMI.
  4. Holds for <seconds> so a human can look at HDMI, then returns.

The CALLER must SIGSTOP the running MiSTer menu process first (so we own the
SPI bus) and SIGCONT it afterwards. HDMI stays live while MiSTer is stopped
because scan-out is FPGA-driven, not CPU-driven.

Usage:  fpga_fbenable_probe.py [seconds]      (default 8)

Recovery if the menu doesn't come back: reboot.
"""
import mmap
import os
import struct
import sys
import time

# --- SoCFPGA manager (GPO/GPI) — see fpga_base_addr_ac5.h / fpga_io.cpp ---
MGR_BASE = 0xFF706000          # SOCFPGA_MGR_ADDRESS
MGR_LEN = 0x1000
GPO_OFF = 0x10                 # write data out  (SOCFPGA_MGR_ADDRESS + 0x10)
GPI_OFF = 0x14                 # read data in    (SOCFPGA_MGR_ADDRESS + 0x14)

SSPI_STROBE = 1 << 17          # fpga_io.cpp
SSPI_ACK = SSPI_STROBE
SSPI_IO_EN = 1 << 20           # spi.cpp
BIT31 = 0x80000000             # "user mode" bit, always set during SPI ops

# --- video_fb_enable constants — video.cpp ---
UIO_SET_FBUF = 0x2F            # user_io.h
FB_EN = 0x8000
FB_FMT_8888 = 0x06
FB_FMT_RxB = 0x10
FB_ADDR = 0x20000000 + (32 * 1024 * 1024)   # 0x22000000
FB_W = 1920
FB_H = 1080

SPIN_LIMIT = 200000            # busy-wait guard so a wrong bit can't hang forever


class Fpga:
    def __init__(self):
        self.fd = os.open("/dev/mem", os.O_RDWR | os.O_SYNC)
        self.m = mmap.mmap(self.fd, MGR_LEN, mmap.MAP_SHARED,
                           mmap.PROT_READ | mmap.PROT_WRITE, offset=MGR_BASE)
        # GPO is write-only (reads return GPI), so we can't recover MiSTer's
        # shadow. Start from BIT31 set / reset+LED clear — the safe idle base.
        self.gpo = BIT31

    def _wr(self, val):
        self.gpo = val & 0xFFFFFFFF
        struct.pack_into("<I", self.m, GPO_OFF, self.gpo)

    def _rd(self):
        return struct.unpack_from("<I", self.m, GPI_OFF)[0]

    def spi(self, word):
        # fpga_spi(): set data, strobe, wait ACK high, drop strobe, wait ACK low
        base = (self.gpo & ~(0xFFFF | SSPI_STROBE)) | (word & 0xFFFF)
        self._wr(base)
        self._wr(base | SSPI_STROBE)
        for _ in range(SPIN_LIMIT):
            gpi = self._rd()
            if gpi & BIT31:
                raise RuntimeError("GPI[31]=1: FPGA uninitialized?")
            if gpi & SSPI_ACK:
                break
        else:
            raise RuntimeError("timeout waiting for ACK high")
        self._wr(base)
        for _ in range(SPIN_LIMIT):
            gpi = self._rd()
            if not (gpi & SSPI_ACK):
                break
        else:
            raise RuntimeError("timeout waiting for ACK low")
        return gpi & 0xFFFF

    def spi_en(self, mask, en):
        gpo = self.gpo | BIT31
        self._wr(gpo | mask if en else gpo & ~mask)

    def enable_io(self):
        self.spi_en(SSPI_IO_EN, 1)

    def disable_io(self):
        self.spi_en(SSPI_IO_EN, 0)

    def fb_enable(self):
        # video_fb_enable(1, 0): one continuous IO transaction.
        # Force a clean chip-select edge first: if MiSTer was SIGSTOPped mid
        # transaction, IO_EN may still be high, so our command word would be
        # mis-parsed as a parameter. Dropping then raising IO_EN resets the
        # FPGA user_io command parser.
        self.disable_io()
        self.enable_io()
        res = self.spi(UIO_SET_FBUF)
        print(f"UIO_SET_FBUF ack = 0x{res:04x} ({'core supports HPS fb' if res else 'NO fb support!'})",
              flush=True)
        fmt = FB_EN | FB_FMT_RxB | FB_FMT_8888       # 0x8016
        fb_addr = FB_ADDR + 4096                     # n==0 => +4096 (params page)
        self.spi(fmt)
        self.spi(fb_addr & 0xFFFF)
        self.spi((fb_addr >> 16) & 0xFFFF)
        self.spi(FB_W)
        self.spi(FB_H)
        self.spi(0)                                  # scaled left
        self.spi(FB_W - 1)                           # scaled right
        self.spi(0)                                  # scaled top
        self.spi(FB_H - 1)                           # scaled bottom
        self.spi(FB_W * 4)                           # stride bytes
        self.disable_io()
        return res


def paint_test_pattern():
    fd = os.open("/dev/fb0", os.O_RDWR)
    stride = FB_W * 4
    size = stride * FB_H
    fb = mmap.mmap(fd, size, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE)
    cell = 120
    # BGRX. Bold checkerboard + white border = obviously not the wallpaper.
    magenta = b"\xff\x00\xff\x00"
    cyan = b"\xff\xff\x00\x00"
    white = b"\xff\xff\xff\x00"
    border = white * 8

    def build_line(parity):
        line = bytearray()
        for cx in range((FB_W + cell - 1) // cell):
            px = magenta if ((cx + parity) & 1) else cyan
            line += px * cell
        line = line[:stride]
        line[:32] = border          # 8px white left border
        line[-32:] = border         # 8px white right border
        return bytes(line)

    even = build_line(0)
    odd = build_line(1)
    white_line = white * FB_W
    rows = []
    for y in range(FB_H):
        if y < 8 or y >= FB_H - 8:
            rows.append(white_line)
        else:
            rows.append(even if ((y // cell) & 1) == 0 else odd)
    fb[:] = b"".join(rows)
    fb.close()                 # MAP_SHARED writes are already in the fb; no flush()
    os.close(fd)
    print("test pattern painted to /dev/fb0", flush=True)


def main():
    seconds = int(sys.argv[1]) if len(sys.argv) > 1 else 8
    paint_test_pattern()
    fpga = Fpga()
    print("issuing video_fb_enable(1, 0)...", flush=True)
    fpga.fb_enable()
    print(f"fb routed; holding {seconds}s — LOOK AT HDMI", flush=True)
    time.sleep(seconds)
    print("done", flush=True)


if __name__ == "__main__":
    main()
