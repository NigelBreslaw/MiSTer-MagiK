# Zero-copy ACP user-attribute fix

## Confirmed cause

The scanout mailbox RBF drove `ARUSER` and `AWUSER` as `5'b00001`.
Cyclone V requires `5'b11111` for coherent, cacheable ACP reads and writes:
bit 0 marks the access shared and bits 4:1 select the matching write-back,
write-allocate inner-cache policy. The old value requested shared but
non-cacheable transactions.

Primary source: Intel, *Cyclone V Hard Processor System Technical Reference
Manual*, section 10.4.2.6, "Control of the AXI User Sideband Signals".

## Before

Device gate `PROD-ZC-ARM-FLUSH-20260711` used the fallback backend for all 78
measured frames:

- `present_backends={'fb0-dirty': 78}`
- `latch_frames=0`
- `fpga_post_delta=0`
- `fpga_flip_delta=0`
- mailbox `active_sequence=0`, `apply_count=0`, `error_count=0`

Artifacts:

- `build/launcher-home-scroll-profiles/PROD-ZC-ARM-FLUSH-20260711-launcher-home-scroll.tsv`
- `build/launcher-home-scroll-profiles/PROD-ZC-ARM-FLUSH-20260711-fpga-latch-before.log`
- `build/launcher-home-scroll-profiles/PROD-ZC-ARM-FLUSH-20260711-fpga-latch-after.log`

## After

The RTL simulation now asserts `AxCACHE=4'b1111` and `AxUSER=5'b11111`, then
passes three alternating descriptor publications and completions (A, B, A),
including torn-publication rejection:

- expected completions: 3
- observed completions: 3
- AXI errors: 0

Command:

```text
scripts/test-fpga-scanout-mailbox.sh
```

Result:

```text
PASS: coherent ACP attributes, B/A/B staging, vblank apply, completion, tear rejection
PASS: Cyclone V bridge wrapper elaborates
```

Hardware AFTER performance remains deliberately unclaimed until CI builds the
new RBF and the three device qualification scenarios complete without fallback
or integrity violations.
