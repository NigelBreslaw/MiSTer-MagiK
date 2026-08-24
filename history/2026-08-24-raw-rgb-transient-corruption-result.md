# Raw-RGB Phase 2 transient corruption — 2026-08-24

The locally signed schema-4 RBF stopped on boot epoch 3, attempt 7 after 46
valid returns. This was the known transient corruption, not a black screen.
No transition or recovery occurred before read-only diagnostics and a native
30-second movie were captured.

Physical frames were byte-identical to the earlier incident:

- primary and confirmation 1:
  `5e106ecf8c4df7585ad8019ff3b7667d7395a29dda6397b2e665916d8f7d9ad1`;
- corrupt confirmation 2:
  `9c55f042898a36f0982736c9fa59e8aeaa139992e1aca4df80e595259052f750`;
- correct authoritative framebuffer:
  `288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250`.

Both failure-time and later live FPGA evidence reported coherent
`raw_rgb_varied`: three active, nonblack, varied frames; stable ownership;
zero latch drops/rejects. The raw response was byte-identical to healthy
attempts immediately before the incident. This proves the raw boundary was not
black or constant, but does not prove complete pixel equality because schema 4
intentionally retains only frame class and first active RGB.

Exact candidate:

- source commit `e5fb64ae230820ed74f02dc528f99766b22940d4`;
- RBF SHA-256
  `3b27159580fcf4071ed78fbf1431fda9fe40d6f4b0c0c88cc7fe40da62db6c70`;
- setup/hold `0.671/0.223 ns`, zero TNS;
- growth `+136` ALMs and `+111` registers, unchanged RAM/DSP/PLL.

Compact machine-readable evidence is in
[`2026-08-24-raw-rgb-transient-corruption-incident-v1.json`](2026-08-24-raw-rgb-transient-corruption-incident-v1.json).
Large artifacts remain ignored under
`build/raw-scaler-rgb-state-phase2-epoch3-07/`.
