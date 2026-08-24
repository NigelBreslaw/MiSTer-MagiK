# Raw-RGB Phase 2 black result — 2026-08-24

The locally signed schema-4 RBF reproduced the genuine persistent MagiK
black-screen failure on boot epoch 7, attempt 8, after 74 valid clean returns.
The campaign stopped immediately. No reboot, launcher restart, RBF reload, or
other recovery occurred before the failure-time record, later live diagnostics,
authoritative framebuffer, native still, and 30-second movie were captured.

## Decisive boundary result

- Physical USB video is uniform video-level black: minimum, maximum, and mean
  luma are all `16`. Its 1920×1080 JPEG SHA-256 is
  `dc4ee4f1eb9ede8c4b031b29fc8ba97d72068a296fb716cec2de087b6b25255e`,
  byte-identical to both earlier confirmed persistent-black incidents.
- The authoritative 960×540 RGB565 latched framebuffer is complete and varied,
  with SHA-256
  `288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250`,
  byte-identical to the known-correct Arcade return in the earlier incidents.
- Both the failure-time and later live schema-4 reads are coherent
  `raw_rgb_black`. All three completed active frames in each read have no
  nonblack pixel, no variation, and first active RGB `0x000000`.
- Ownership is stable. Failure-time latch status has zero drops and rejects;
  the later live record remains black while valid posts and flips advance.
- Main and the launcher remain healthy and revealed in the Arcade screen with
  input enabled and `present_status: "ok"`.

This is the first direct evidence that the black frame already exists at the
raw ascal RGB output. Together with the correct latched framebuffer and the
earlier schema-3 result showing stable raw CE/DE/HS/VS control, it localizes the
persistent black-screen mechanism to the scaler's framebuffer fetch, returned
pixel-data, or reset/traffic epoch path. It rules out a fault created only by
the downstream OSD/final HDMI output path for this incident.

The result does not yet distinguish a stalled/lost read-credit condition from
valid reads returning or selecting zero data. The next experiment should be a
single passive probe at the ascal memory-return/pixel-consumption boundary,
retaining the queued-completion repair and leaving latch-v5 untouched.

## Frozen identity

- source commit `e5fb64ae230820ed74f02dc528f99766b22940d4`;
- RBF SHA-256
  `3b27159580fcf4071ed78fbf1431fda9fe40d6f4b0c0c88cc7fe40da62db6c70`;
- setup/hold `0.671/0.223 ns`, zero TNS;
- growth `+136` ALMs and `+111` registers, unchanged RAM/DSP/PLL.

Compact machine-readable evidence is in
[`2026-08-24-raw-rgb-black-incident-v1.json`](2026-08-24-raw-rgb-black-incident-v1.json).
Large artifacts remain ignored under
`build/raw-scaler-rgb-state-phase2-epoch7-08/`.
