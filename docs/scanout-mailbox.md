# Scanout mailbox ABI

MiSTer MagiK uses one 4 KiB cache-coherent mailbox for scanout control. The
mailbox is the only traffic sent through the Cyclone V Accelerator Coherency
Port (ACP). Pixel buffers remain ordinary software-managed DMA memory and are
never read or written through the ACP.

The FPGA-to-HPS bridge is 128 bits wide (`port_size_config=2'b10`). Every
mailbox transfer is one aligned 128-bit AXI3 beat with `AxCACHE=4'b1111`,
`AxPROT=3'b000`, and `AxUSER=5'b00001`. Cyclone V passes FPGA-to-HPS AxUSER
through the ACP ID mapper. The mailbox must be below 1 GiB so its physical
address is in ACP mapper page 0. The driver must reject an allocation outside
that window; it must not alter the system-wide ACP mapper to accommodate one.

## Cache-line layout

All integers are little-endian. Each structure owns a separate 64-byte cache
line even though the FPGA reads or writes only the populated prefix.

| Offset | Owner | Purpose |
| --- | --- | --- |
| `0x000` | CPU | Control/commit |
| `0x040` | CPU | Descriptor A |
| `0x080` | CPU | Descriptor B |
| `0x0c0` | FPGA | Completion/fence |

Control is one 128-bit beat:

| Bits | Field |
| --- | --- |
| 31:0 | `0x4d475343` (`MGSC`) |
| 63:32 | reset epoch |
| 95:64 | committed sequence |
| 96 | descriptor index (A=0, B=1) |
| 127:97 | zero |

Each descriptor is two 128-bit beats. Beat 0 contains descriptor magic
`0x4d474452` (`MGDR`), epoch, sequence, and framebuffer physical base. Beat 1
contains format `[5:0]`, filter `[6]`, enable `[7]`, slot `[9:8]`, width
`[27:16]`, height `[43:32]`, byte stride `[61:48]`, hmin `[75:64]`, hmax
`[91:80]`, vmin `[107:96]`, and vmax `[123:112]`.

Completion is one FPGA-owned beat containing magic `0x4d47434d` (`MGCM`),
epoch, active sequence, and status. Status reports the pending bit, enable bit,
and active slot.

## Publication protocol

1. CPU owns an inactive scanout slot and renders into it.
2. CPU publishes the matching descriptor, then publishes the control sequence
   last with release ordering.
3. FPGA reads control, reads both descriptor beats, and re-reads control.
4. FPGA stages the descriptor only when both control reads and the descriptor
   have identical non-stale epoch/sequence values. It does not replace an
   already-pending descriptor.
5. FPGA applies the staged route only on HDMI vblank, then writes completion.
6. Completion sequence is the slot-release fence. CPU must not make that slot
   writable again before observing it with acquire ordering.

UIO command `0x59` bootstraps the aligned mailbox base and epoch; the final
payload word `0x4d49` arms polling. Beginning a new bootstrap disables the old
session first. Command `0x5a` reports magic `0x4d4a`, ABI/capabilities, active
and pending sequences, slot/state, apply/error counters, and epoch. Existing
`0x57`/`0x58` vblank-latch commands remain available as the compatibility
fallback for one release.

## Evidence basis

The wiring follows Intel's Cyclone V HPS register map and design guidance:
the ACP ID mapper is at `0xff707000`, FPGA soft-IP is an F2H master, F2H AxUSER
passes through, and ACP is intended for small coherent control data rather than
large bulk transfers. The bridge primitive signature and width encoding were
also checked against Platform Designer-generated Cyclone V RTL. Local protocol
validation is run by `scripts/test-fpga-scanout-mailbox.sh`.
