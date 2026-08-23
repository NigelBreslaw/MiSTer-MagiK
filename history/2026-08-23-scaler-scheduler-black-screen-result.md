# Minimal scaler-scheduler diagnostic result

## Result

On 2026-08-23 the locally signed-off experimental diagnostic RBF reproduced
the persistent physical black screen on the third valid `launch-return-once`
transition. The first two transitions passed. The campaign stopped
immediately, and no restart, recovery, RBF reload, or further transition was
performed before two additional read-only snapshots were captured.

The failed transition's native `USB Video` frame was uniform 1920x1080
video-level black (`minimum=16`, `maximum=16`, `mean=16`). At the same
checkpoint the authoritative hidden-slot RGB565 framebuffer contained the
correct, nonblank Arcade view. Main and the launcher remained ready, the HDMI
route remained selected, input was enabled, and latch flips continued without
drops or rejects.

## Exact candidate

- FPGA implementation commit: `612a04a0f`
- experimental path-count exception commit: `aba5f27a2`
- host diagnostic decoder commit: `d903ea217`
- RBF SHA-256:
  `3701ec7e5ef7be168bc221fe208f41e8035e60d31d308ed3ecafcbb9a96ffde0`
- metadata SHA-256:
  `b6cd1cbfe0edeeaefc31aa8c31937c51ea3e5535980df4ac4798023517a8b462`
- latch protocol/capabilities: `5` / `0x03ff`
- Main PID/generation: `2745` / `424131`
- launcher PID/owner epoch: `2763` / `1`

The canonical local Quartus 17.0 Build 595 seed-2 signoff measured setup
`0.660 ns`, hold `0.249 ns`, zero TNS, `+131` ALMs, `+89` registers, unchanged
RAM/DSP/PLL identity, and all 26 declared diagnostic CDC paths. The operator
accepted only the diagnostic output-path count of 160 instead of the 158
production baseline; no functional, timing, CDC, MTBF, resource, or warning
gate was waived.

## Preserved evidence

The failed benchmark directory is retained locally at:

```text
build/agent-benchmarks/launch-return-once/1787492611
```

| Artifact | SHA-256 | Result |
| --- | --- | --- |
| `summary.json` | `6863f6c3a411d0b0df4eb9da3b050748e5eb365ae31708ba55ef699b26e340d6` | failed closed |
| `returned-usb-video.jpg` | `dc4ee4f1eb9ede8c4b031b29fc8ba97d72068a296fb716cec2de087b6b25255e` | uniform physical black |
| `returned-framebuffer.png` | `288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250` | correct Arcade return |
| `snes-view-framebuffer.png` | `66314b11ea3affacc297982b9f0c94376f1ad22f7bf0e83f26351ee926cd4ede` | correct settled SNES view |

The benchmark's first scheduler capture and both subsequent read-only captures
each contained three identical, CRC-valid `0x67` records. The two compact
post-failure records are committed beside this note. Their source hashes were:

- first follow-up:
  `935964ddc40381fb3b719903ece448b56843b6749a8bf46f4bbe4fc4ae05f69e`
- second follow-up:
  `91d634e00dcb2762a9d8e46e0cd5d5c601d700fb61ffed8ad281d0decd7c51a3`

## Scheduler evidence

All nine reads across the three captures decoded to the same 16-bit state word
`0x0da3`:

| Field | Value |
| --- | --- |
| coherent/running | true / true |
| read accepted/completion seen in frame | false / false |
| read level/copy level | 2 / 2 |
| request/pending/acknowledgement/destination seen | 1 / 0 / 1 / 1 |
| reset-return drain/credits/phase | 0 / 0 / 0 |

The request toggle, synchronized acknowledgement, and destination-observed
toggle agree. No completion is pending, and reset-return accounting is empty.
This occurrence is therefore not a completion-queue backlog and not the
specified `readlev=2`, `copylev=0` credit-accounting stall. The conservative
host classification is `scaler_scheduler_not_stalled` because both scheduler
levels remain populated; this record does not claim physical visibility.

## Decision

The queued completion repair remains justified and unchanged, but it is not
the root cause of this captured occurrence. The minimal probe has answered its
question and should now be retired rather than widened in place.

Per the predeclared decision tree, the next diagnostic—if authorized—should be
a separate minimal read-only probe at the raw scaler boundary. It should
distinguish whether the scaler is emitting black pixels with valid timing or
whether progress stops between the populated scheduler and raw scaler output.
It must not alter latch-v5, the repaired completion transport, framebuffer
routing, reset control, PLL, mux, or final pixel logic. No further RBF change or
device recovery is part of this incident-capture step.
