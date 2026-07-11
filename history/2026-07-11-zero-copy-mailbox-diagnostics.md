# Zero-copy mailbox diagnostics

## Confirmed cause

The existing latch report exposed only the legacy direct-latch counters. It
could not distinguish renderer fallback, kernel ownership stalls, or an FPGA
mailbox that never observed a post. The slot-mismatch error likewise omitted
the slot actually rendered by Slint.

## Before

- Mailbox sequence/apply/error fields in `fpga-latch-report`: 0 fields.
- Slot mismatch identified rendered and selected slots: 0 of 2.

## After

- Mailbox sequence/apply/error/epoch fields: 8 fields reported.
- Slot mismatch identifies both rendered and selected slots: 2 of 2.
- Evidence isolated the current issue: FPGA applied sequence 2 with zero FPGA
  errors while the kernel presenter retained a pending fence.

Evidence:

- `build/launcher-home-scroll-profiles/PROD-ZC-COHERENT-20260711-fpga-latch-after.log`
- `build/launcher-home-scroll-profiles/PROD-ZC-COHERENT-20260711-launcher-home-scroll.log`

Validation:

- Repository pre-commit host tests, checks, and clippy.
- ARM `release-device` build with diagnostics enabled.
