#!/usr/bin/env python3
"""Read-only probe: ask the FPGA for its current video + framebuffer config.

Replays MiSTer's UIO_GET_VRES (0x23) and UIO_GET_FB_PAR (0x40) read sequences
so we learn the EXACT geometry MiSTer uses for the (correctly-displayed) menu
wallpaper. We then feed those values into the fb-enable probe instead of
guessing 1920x1080.

For each SPI word it captures the GPI value both while STROBE is high and after
STROBE drops, because we saw read data is presented during the ack-high window.
This also tells us, once and for all, which phase the real data lives in.

Run with MiSTer SIGSTOPped so we own the SPI bus. Pure reads; changes nothing.
"""
import mmap
import os
import struct
import sys

MGR_BASE = 0xFF706000
MGR_LEN = 0x1000
GPO_OFF = 0x10
GPI_OFF = 0x14
STROBE = 1 << 17
ACK = STROBE
IO_EN = 1 << 20
BIT31 = 0x80000000
LIMIT = 200000

UIO_GET_VRES = 0x23
UIO_GET_FB_PAR = 0x40


class Fpga:
    def __init__(self):
        self.fd = os.open("/dev/mem", os.O_RDWR | os.O_SYNC)
        self.m = mmap.mmap(self.fd, MGR_LEN, mmap.MAP_SHARED,
                           mmap.PROT_READ | mmap.PROT_WRITE, offset=MGR_BASE)
        self.gpo = BIT31

    def _wr(self, v):
        self.gpo = v & 0xFFFFFFFF
        struct.pack_into("<I", self.m, GPO_OFF, self.gpo)

    def _rd(self):
        return struct.unpack_from("<I", self.m, GPI_OFF)[0]

    def xfer(self, word):
        """One SPI word; return (hi, lo) = low16 captured at ack-high / ack-low."""
        base = (self.gpo & ~(0xFFFF | STROBE)) | (word & 0xFFFF)
        self._wr(base)
        self._wr(base | STROBE)
        hi = None
        for _ in range(LIMIT):
            g = self._rd()
            if g & ACK:
                hi = g & 0xFFFF
                break
        self._wr(base)
        lo = None
        for _ in range(LIMIT):
            g = self._rd()
            if not (g & ACK):
                lo = g & 0xFFFF
                break
        return hi, lo

    def enable_io(self):
        self._wr((self.gpo | BIT31) | IO_EN)

    def disable_io(self):
        self._wr((self.gpo | BIT31) & ~IO_EN)

    def cmd_read(self, cmd, n_words):
        # clean chip-select edge, send command, then read n response words
        self.disable_io()
        self.enable_io()
        chi, clo = self.xfer(cmd)
        words = [self.xfer(0) for _ in range(n_words)]
        self.disable_io()
        return (chi, clo), words


def show(label, cmd_ack, words):
    print(f"\n=== {label} ===", flush=True)
    print(f"  cmd ack: hi=0x{cmd_ack[0]:04x} lo=0x{cmd_ack[1]:04x}", flush=True)
    for i, (hi, lo) in enumerate(words):
        print(f"  w{i:<2} hi=0x{hi:04x} ({hi:5d})   lo=0x{lo:04x} ({lo:5d})", flush=True)


def main():
    f = Fpga()
    ack, words = f.cmd_read(UIO_GET_VRES, 16)
    show("UIO_GET_VRES (0x23)  [w0=flags, w1/w2=width, w3/w4=height, ...]", ack, words)
    ack, words = f.cmd_read(UIO_GET_FB_PAR, 6)
    show("UIO_GET_FB_PAR (0x40) [arx, ary, fb_fmt, fb_width, fb_height, ...]", ack, words)
    f.disable_io()
    print("\ndone", flush=True)


if __name__ == "__main__":
    sys.exit(main())
