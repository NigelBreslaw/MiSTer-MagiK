# FPGA scanout mailbox evidence — 2026-07-11

## Confirmed cause

The proof path used two separate operations: Linux synchronized memory, then
userspace sent UIO command `0x57` to arm a route. That gap cannot transfer DMA
ownership and arm a vblank latch atomically, and the FPGA had no coherent
descriptor/fence channel back to the driver.

## Before / after

- Before: 0 coherent mailbox ABIs, 0 atomic descriptor-to-vblank completion
  paths, and split sync/post control.
- After: 1 versioned mailbox ABI, two alternating descriptor lines, one
  FPGA-owned completion line, stable control re-read, vblank-only apply, and
  one compatibility UIO path retained.
- Performance p99 remains the production baseline at this commit because no
  userspace path selects the mailbox yet: Home 6,888 us; Arcade 3,736 us;
  preview 2,469 us. The final default commit must beat all three.

## Tests

`scripts/test-fpga-scanout-mailbox.sh`

Result:

```text
PASS: coherent mailbox stage, vblank apply, completion, tear rejection
PASS: Cyclone V bridge wrapper elaborates
```

The build patch was also applied in order against pinned Menu_MiSTer commit
`cf4dfdee516fcaa6952bdd9fb47154e96c28567e`; it removes the disabled F2H hard-IP
instance before adding the mailbox-owned 128-bit instance.

## Evidence artifacts

- `docs/scanout-mailbox.md`
- `experiments/fpga-vblank-latch/mister_magik_scanout_mailbox.sv`
- `experiments/fpga-vblank-latch/tb_mister_magik_scanout_mailbox.sv`
- `experiments/fpga-vblank-latch/Menu_MiSTer-scanout-mailbox.patch`
- `scripts/test-fpga-scanout-mailbox.sh`
- `history/2026-07-11-production-zero-copy-baseline.md`

## Current Menu master patch repair

Manual workflow run `29144143796` passed the mailbox RTL test but failed before
Quartus because the legacy latch patch had a duplicated `io_dout_sys` context
line and omitted Menu master's newer `sl_r` and command `0x44` lines. The
scanout patch likewise omitted `sl_r` from its insertion context. Both patches
were recounted and verified in order against Menu_MiSTer commit
`3c3634c0105d78f27aeba66b38966c50dbc42c9b`.

This repair changes no generated logic or performance metric. Before: zero RBF
artifacts from run `29144143796`. After locally: both patches apply in order and
the mailbox RTL/bridge simulation passes. A replacement CI RBF remains required
before device qualification.
