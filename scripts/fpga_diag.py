#!/usr/bin/env python3
"""Diagnostic for the MiSTer HPS<->FPGA SPI bridge (GPO/GPI in the FPGA manager).

Read-only-ish: it toggles IO_EN + one strobe to see whether the ACK handshake is
real, then restores. Run with MiSTer SIGSTOPped so we own the bus. No fb writes.

Prints:
  - idle GPI reads (is the bridge connected? what bits are set?)
  - io version / fio size / core-id-ish bits decoded
  - an instrumented single SPI word (cmd 0x2F): iteration counts + raw GPI trace
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


def main():
    fd = os.open("/dev/mem", os.O_RDWR | os.O_SYNC)
    m = mmap.mmap(fd, MGR_LEN, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE, offset=MGR_BASE)

    def wr(v):
        struct.pack_into("<I", m, GPO_OFF, v & 0xFFFFFFFF)

    def rd():
        return struct.unpack_from("<I", m, GPI_OFF)[0]

    print("=== idle GPI (8 reads) ===", flush=True)
    for _ in range(8):
        g = rd()
        print(f"  gpi=0x{g:08x}  bit31={ (g>>31)&1 } ack(bit17)={ (g>>17)&1 } iover={ (g>>18)&3 } fio={ (g>>16)&1 }", flush=True)

    gpo = BIT31
    print(f"\n=== set base GPO=0x{gpo:08x} (BIT31) then EnableIO (|IO_EN) ===", flush=True)
    wr(gpo)
    print(f"  after base write: gpi=0x{rd():08x}", flush=True)
    wr(gpo | IO_EN)
    gpo = gpo | IO_EN
    for _ in range(4):
        print(f"  after IO_EN:      gpi=0x{rd():08x}", flush=True)

    word = 0x2F
    base = (gpo & ~(0xFFFF | STROBE)) | word
    print(f"\n=== instrumented SPI word 0x{word:04x} (base GPO=0x{base:08x}) ===", flush=True)
    wr(base)
    print(f"  data set, strobe low:  gpi=0x{rd():08x}", flush=True)
    wr(base | STROBE)
    trace = [rd() for _ in range(10)]
    print("  strobe HIGH, first 10 gpi: " + " ".join(f"0x{t:08x}" for t in trace), flush=True)
    # count until ACK high
    hi = 0
    while hi < 200000 and not (rd() & ACK):
        hi += 1
    print(f"  iterations until ACK high: {hi} (gpi=0x{rd():08x})", flush=True)
    wr(base)
    trace2 = [rd() for _ in range(10)]
    print("  strobe LOW, first 10 gpi:  " + " ".join(f"0x{t:08x}" for t in trace2), flush=True)
    lo = 0
    while lo < 200000 and (rd() & ACK):
        lo += 1
    final = rd()
    print(f"  iterations until ACK low:  {lo} (gpi=0x{final:08x}) data=0x{final & 0xFFFF:04x}", flush=True)

    # DisableIO
    wr((gpo & ~IO_EN) | BIT31)
    print("\n=== restored (IO_EN cleared) ===", flush=True)
    m.close()
    os.close(fd)


if __name__ == "__main__":
    sys.exit(main())
