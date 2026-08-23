# Phase 2 transient physical corruption — 2026-08-24

## Result

The corrected direct-Arcade Phase 2 campaign stopped on boot epoch 4,
attempt 15 after 14 valid passes. This was not a black screen and not a
persistent moving-band state. It was one sampled transient physical-output
corruption between two byte-identical healthy MagiK frames.

The test used the same fixed 1943 MRA and launch/handoff/typed-return path as
the earlier confirmed-black reproducer, without its unnecessary SNES
navigation. The game remained active for a recorded 2,000 ms after proven
handoff. Physical MagiK output was then sampled three times over 3,283 ms.

| Sample | SHA-256 | Luma min/max/mean | Row discontinuity | Result |
|---|---|---|---:|---|
| Primary | `5e106ecf8c4df7585ad8019ff3b7667d7395a29dda6397b2e665916d8f7d9ad1` | `16/235/54` | `74‰` | healthy Arcade |
| Confirmation 1 | `9c55f042898a36f0982736c9fa59e8aeaa139992e1aca4df80e595259052f750` | `0/255/44` | `92‰` | corrupted |
| Confirmation 2 | `5e106ecf8c4df7585ad8019ff3b7667d7395a29dda6397b2e665916d8f7d9ad1` | `16/235/54` | `74‰` | healthy Arcade |

The authoritative 960x540 RGB565 framebuffer was complete and correct with
SHA-256
`288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250`.
The first and third physical frames are byte-identical, while the middle
physical frame visibly differs. This rules out an intended launcher update.

## FPGA evidence

The failure-time and later unrecovered-state snapshots both reported:

- diagnostic architecture `raw-scaler-boundary-v1`;
- coherent classification `raw_scaler_active`;
- advancing completed-frame sequence;
- clock enable, horizontal sync, saturated active samples, and saturated
  nonzero samples in all three records;
- `LauncherActive`, stable MagiK ownership, and RGB565 route 960x540;
- zero latch drops and zero latch rejects;
- `sink_visibility: "unobserved"` for the FPGA observer.

The passive observer therefore does not show a raw-scaler activity stall. It
cannot establish correct line count, HS/VS/DE ordering, active width, or phase,
so it does not distinguish malformed raw timing from a defect farther
downstream.

## Native movie follow-up

A 30-second movie was recorded with the macOS native USB Video path before any
device recovery. AVFoundation decoded and inspected every delivered frame:

- 755 frames covering 30,160 ms;
- luma mean `54` in every frame;
- row-discontinuity metric `73‰` in every frame;
- only normal decoder-range variation in sampled extrema (`14..15` minimum,
  `237..240` maximum).

The corruption had therefore self-cleared before the preservation movie. The
movie does not extend the corrupt interval; it proves that the incident was
transient rather than the previously recorded continuously moving state.

## Bounded conclusion

This is valid physical-output evidence but not the requested black-screen
reproduction. It supports a transient timing/phase-integrity failure after the
authoritative framebuffer and while substantial raw-scaler activity continues.
It does not prove that the corruption and black-screen mechanisms are the same.

No reboot, launcher restart, RBF reload, or recovery occurred after detection.
Large images and the 14 MB movie remain ignored under
`build/raw-scaler-phase2-epoch4-15/`. Their compact integrity record is
[`2026-08-24-phase2-transient-corruption-incident-v1.json`](2026-08-24-phase2-transient-corruption-incident-v1.json).
