# HDMI lock-high black-screen evidence — 2026-08-13

This note preserves the first rare return-to-MagiK black screen captured with
the qualified lock-only FPGA diagnostic still readable. The MiSTer was not
rebooted, reconfigured, delivered to, or otherwise mutated while collecting
the evidence.

## Observations

- The fixed `USB Video` input did not produce a visible frame during the
  bounded three-second capture window.
- The authoritative FPGA-latched scanout-slot capture contained the expected
  960x600 MiSTer MagiK home screen with varied, non-black content.
- Main and the launcher remained alive and coherent. The launcher reported
  `present_backend=fpga-vblank-latch-hidden`, `present_status=ok`,
  `display_frozen=false`, and a fresh VS period of approximately 16.659 ms.
- Latch evidence reported active sequence 5, flip count 5, post count 5, and
  drop count 0.
- `hdmi-lock-evidence-v1` remained available and coherent. The real physical
  FPLL evidence was armed and currently locked.
- The sticky lock-loss count was 1, exactly matching the count captured before
  this black-screen occurrence. The occurrence therefore did not add a PLL
  lock loss.

The four raw lock-evidence words were:

```text
0001 000f 0001 cfb6
```

## Conclusion

This occurrence rules out a new physical HDMI FPLL unlock as its cause. It
also proves that valid MagiK pixels existed in the authoritative scanout slot
while the external HDMI capture was black. The remaining boundary is between
the final registered FPGA HDMI output and the downstream transmitter, PHY,
cable, capture device, or display.

The next diagnostic milestone is therefore deliberately narrow: observe only
the final registered `HDMI_TX_VS`, `HDMI_TX_DE`, and `HDMI_TX_D` sources and
report completed-frame activity as no-DE, DE-with-all-zero-active-RGB, or
DE-with-at-least-one-nonzero-active-RGB. The permanent lock-only `0x60`
contract remains unchanged. The new evidence must use single-bit event CDC,
the existing read-only response path, and no functional video fanout.

This evidence does not claim that a nonzero final pixel is the expected pixel;
it distinguishes an entirely black final FPGA stream from some active digital
video. Expected-pattern or signature comparison remains a later,
evidence-triggered milestone.
