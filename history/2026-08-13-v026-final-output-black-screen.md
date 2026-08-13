# v0.26 final-output black-screen evidence — 2026-08-13

This note preserves the first rare Arcade-to-MagiK black screen captured with
the qualified final-output FPGA recorder. The MiSTer was not rebooted,
reconfigured, delivered to, or otherwise mutated while collecting the
evidence.

## Qualified state

- Platform release: v0.26.
- MagiK source revision: `13ac34ece61425b7b1fd6ff4641675b66729b4f7`.
- Main source revision: `7788f535e66260c2c1d5bc23977accf087dc6bf4`.
- Main's later return-output reassertion fix
  `c563d96028e406f6f89db010055e65f4f7d0bba8` was not present in the
  installed platform.
- FPGA diagnostics were available, coherent, and stable across owner epoch 1.

## Captured evidence

- Classification: `final_output_de_all_zero`.
- The final registered FPGA HDMI output completed three frames in 50.094 ms.
  Every frame contained DE but no nonzero active RGB sample.
- The physical HDMI FPLL was armed and currently locked. Its sticky loss count
  was 1, unchanged from the preceding working state.
- The active latch route was enabled and MagiK-owned (`flags=0x0009`) at
  physical base `0x22fd2000`, 960x600, stride 1920.
- Active sequence, post count, and flip count were all 6; pending sequence and
  drop count were zero.
- A read-only capture of that exact scanout slot contained the correct,
  colourful MagiK home screen.

The retained diagnostic JSON SHA-256 was
`d753a5feaefca81d5ebb38d703ab483d8dadac702b877c1581e26a22fac4bfb6`.
The local framebuffer capture SHA-256 was
`5472a5bd65619df491f6efbf8e88c3e73a84882b1eefbf345b9189f5f52e3e55`;
the screenshot itself is not repository evidence and was not staged.

## Proven boundary

The failure originates before the final FPGA HDMI output registers. It is not
an external-only transmitter, PHY, cable, capture-device, or display failure:
the FPGA itself supplied zero RGB during active DE. It is also not a new FPLL
unlock, a missing source framebuffer, or a rejected latch post.

The remaining live fork is:

1. stale FPGA route state selected the intentionally black native/direct
   source;
2. the scaled route was selected but `ascal` produced black or no raster;
3. `ascal` produced pixels that were lost in shadowmask/OSD or final staging;
4. the scaler's Avalon framebuffer requests stalled or did not return data.

The correct next diagnostic release therefore observes all four boundaries in
one passive build: cycle-aligned final mux provenance, raw `ascal` activity,
post-OSD activity, and bucketed Avalon request/accept/return liveness. Only
single-bit event toggles cross clock domains. Pixel data, addresses, and
functional control never cross into or out of diagnostics.

Main `c563d960` must ship in the same candidate. It reasserts runtime
output/scaler configuration on launcher return and may prevent the stale-state
fault. The FPGA evidence exists to localize any occurrence that survives that
fix; it is not itself a video-path repair.
