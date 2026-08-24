# Raw-scaler frame-integrity Phase 2 black result — 2026-08-24

## Result

The locally signed schema-3 diagnostic RBF reproduced the genuine persistent
MagiK black-screen failure on boot epoch 3, attempt 5. The campaign stopped
immediately. The device remains unrecovered: no subsequent transition,
launcher restart, RBF reload, or reboot has occurred.

Before the failure, 44 valid direct-Arcade launch/return transitions passed
across three boot epochs, with a supervised reboot after each group of 20.
Every pass reported visible physical MagiK output and
`raw_control_stable_since_baseline`.

## Physical and internal evidence

- The 1920×1080 native USB-video still is uniform video-level black:
  minimum, maximum, and mean luma are all `16`.
- Its SHA-256 is
  `dc4ee4f1eb9ede8c4b031b29fc8ba97d72068a296fb716cec2de087b6b25255e`,
  byte-identical to the 2026-08-23 confirmed black incident.
- The authoritative 960×540 RGB565 framebuffer is complete, varied, and has
  SHA-256
  `288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250`,
  byte-identical to the known-correct Arcade return in the earlier incident.
- A 30-second native AVFoundation movie was recorded after failure without
  recovering the device. It remains ignored under the incident artifact root.
- A subsequent native visible-frame request timed out after three seconds,
  confirming that the physical black state had not recovered.

## FPGA evidence

Both the failure-time evidence and the later live snapshot report:

- architecture `raw-scaler-frame-integrity-v1`;
- classification `raw_control_stable_since_baseline`;
- baseline control CRC `45489` and first-bad CRC `0`;
- three identical, coherent records;
- stable MagiK ownership and `LauncherActive`;
- zero latch drops and zero latch rejects;
- `sink_visibility: "unobserved"`.

The observer fingerprints the ordered CE/DE/HS/VS stream but deliberately does
not observe RGB. This incident therefore provides strong evidence that the
black-screen mechanism is not a raw-scaler control-waveform change at this
boundary. It does not distinguish corrupt/zero RGB from a defect after that
boundary.

## Frozen identity and decision

- signed FPGA source commit:
  `20164bf981ed4950370ca3595427b73da2eb4afa`;
- RBF SHA-256:
  `d0db56092eb652e8f51e4cf42cd8174d725f92b47e61384e140ec67f2898528f`;
- local signoff: setup `0.516 ns`, hold `0.224 ns`, zero TNS;
- runtime launcher: `0.2.5219`, build 5219, source
  `aa954e639ee3461c90f0420f9eed56ec00e6b637`;
- Main PID/generation: `2748` / `161924`;
- launcher PID: `2775`;
- owner epoch: `1`.

Retire the raw-control probe for the next diagnostic candidate. Preserve the
queued-completion repair and latch-v5 unchanged. The next experiment should be
one passive sticky fingerprint at the next downstream boundary, sufficient to
separate raw RGB corruption from later HDMI-path corruption. Do not broaden it
into a general observer and do not infer physical visibility from FPGA data.

Compact evidence is retained in
[`2026-08-24-frame-integrity-black-incident-v1.json`](2026-08-24-frame-integrity-black-incident-v1.json).
Large media remains ignored under
`build/raw-scaler-frame-integrity-phase2-epoch3-05/`.
