# Reproduced physical signal loss after Arcade return

## Summary

On 2026-08-23 the fixed `launch-return-once` benchmark completed one real
Arcade launch and return on the unchanged `platform-v0.29` FPGA platform. The
internal RGB565 framebuffer was correct and the latch transport remained
coherent, but the fixed `USB Video` capture showed the capture card's exact
eight-bar signal-loss pattern instead of MiSTer MagiK. Two stills taken 23.99
seconds apart were byte-identical.

This run must not be counted as a physical-video pass. The benchmark's old
luma-only classifier called any sufficiently nonblack frame `visible`, so it
incorrectly accepted the signal-loss bars. The classifier now identifies this
fixed pattern as `signal_lost`; the same preserved input then timed out waiting
for a visible frame.

Immediately before the transition, the same fixed `USB Video` device produced
a clean MiSTer MagiK frame. That ignored local control image has SHA-256
`2585e70a4700123f7065c8951e70e0101747aac4770cff2c033ffe77eda17cf7`.
This excludes selection of an unrelated camera or a permanently disconnected
capture input as the explanation for the post-transition bars.

The evidence places the failure beyond the application framebuffer and latch
transport. It is consistent with absent or invalid HDMI at the capture-card
input, but the capture card alone cannot distinguish a MiSTer HDMI/TMDS fault
from a capture-device input fault. A correlated display or second analyzer is
still required for that distinction.

## Exact software and platform identity

- application source revision:
  `5e176f23babbb6f24d0a8c7662a91e5ed30db664`
- application build: `0.2.5184`, clean ARM build
- platform release: `platform-v0.29`
- RBF SHA-256:
  `7484e004b3c6e089d9d377658633e435703bc1a224943b06215df9a9bccef4e7`
- platform manifest SHA-256:
  `87d0fd7c8314b5f5154d06122bd28a7ba9ca42fdd0aec3d3149490d61257f215`
- platform bundle ID:
  `67c943bddf3325f82d6e6666f6046b16dab9d5a972295b0167054b181443170e`
- qualification candidate ID:
  `b83a3a0b696b3cbe7cc6331c4ff49fbb1a8ba1bda4e1c7670ef67e0dd0f79105`

The RBF, Main platform component, latch protocol, and scanout module were not
changed by this diagnostic run. Runtime delivery updated only the application
binary and manifest before the benchmark.

## Reproduction and preserved state

The run used:

```text
scripts/agent benchmark launch-return-once
```

The benchmark summary is retained locally under
`build/agent-benchmarks/launch-return-once/1787436067/summary.json`. It recorded:

- one completed return with the expected Arcade selection restored;
- a correct internal framebuffer for the returned Arcade UI;
- `present_backend=fpga-vblank-latch-hidden` and `present_status=ok`;
- zero latch drops and no latch failure state;
- FPGA classification `repair_transport_ready`, coherent state, and
  `sink_visibility=unobserved`;
- a 1920x1080 USB frame incorrectly classified as visible.

Manual inspection showed that USB frame was the signal-loss bar pattern. The
device was then frozen in place: no reboot, RBF reload, launcher restart,
display-mode change, or additional launch/return transition was performed.
Two typed read-only bundles were collected with:

```text
scripts/agent device diagnostics --out build/black-screen-20260823-live-1
scripts/agent device diagnostics --out build/black-screen-20260823-live-2
```

The exact FPGA records are committed alongside this note:

- `2026-08-23-color-bars-signal-loss-fpga-1.json`
- `2026-08-23-color-bars-signal-loss-fpga-2.json`

## Diagnostic delta

The snapshots were 23.99 seconds apart in device monotonic time.

| Field | First | Second | Delta |
| --- | ---: | ---: | ---: |
| owned vblank count | 63,390 | 64,829 | +1,439 |
| presented vblank count | 461 | 462 | +1 |
| repeated vblank count | 62,929 | 64,367 | +1,438 |
| active sequence | 461 | 462 | +1 |
| post count | 461 | 462 | +1 |
| flip count | 461 | 462 | +1 |
| drop count | 0 | 0 | 0 |
| reject count | 0 | 0 | 0 |
| ownership loss count | 0 | 0 | 0 |

The owned-vblank rate was approximately 60 Hz. Owner epoch remained `1`,
MagiK ownership remained stable, and the lifetime invariant remained valid.
The active route was 960x540 RGB565 with 1,920-byte stride. Main reported zero
crashes, restarts, invariants, blocked SPI writes, and blocked GPO writes.

The two physical USB stills were byte-identical with SHA-256
`33779a140586055e719b717b3dd4b5d93b4c214e8e18955fb98da96a4366dd0e`.
They remain ignored local evidence and are not committed.

## Source hashes

| Snapshot | File | SHA-256 |
| --- | --- | --- |
| first | `fpga-video-diagnostics.json` | `415c6f52ac4bc017e654aac785645e49a2a22df969144b0e24574723428cdade` |
| second | `fpga-video-diagnostics.json` | `1005a6fc5b3d2be36f665f4952e0b911824c055dc78c236466e709e17d501bee` |
| first | `main-status.json` | `994accc553e60d7260722e5a5994e89a9c7e9b71bcaa60c7e5590b7d351f987b` |
| second | `main-status.json` | `fbc2a63324249f533be738b500c11c3642099e4be68b137f0691b2ee149986aa` |
| first | `slint-status.json` | `058a144ced455fff709b5ffcc8cb9406ecd15e137840ef02e46783bd56b60a0c` |
| second | `slint-status.json` | `8c0d8c6ec8f710ae4c43def66197745f8d83e1138dff10f96f3369973390ebf7` |
| benchmark | `summary.json` | `90c8440f01ed677c014a6db43d3f31932369f9a17a9ddb90bf8f96e288313d73` |

## Design implication

The next diagnostic RBF should remain passive and isolated from the working
latch protocol. It needs enough read-only evidence to separate these remaining
boundaries during a preserved incident:

- scaler fetch requests, accepts, returns, and completion-credit state;
- scaler HDMI-domain progress and output pixel/DE/HS/VS counters;
- final pre-TMDS RGB/DE/HS/VS fingerprints;
- HDMI clock, PLL-lock, output-enable, reset, and transmitter-state evidence.

No observer result should be described as sink visibility. A second physical
sink/analyzer correlation remains the authority for whether the failure is in
MiSTer output or the USB capture input.
