# USB temporal detector v2 — 2026-08-25

Phase 2 return 45 was a detector false positive. The primary and first
confirmation were byte-identical, complete Arcade launcher frames with raw luma
range `16..235` and mean 54. The second confirmation was also a complete normal
launcher frame, but AVFoundation delivered raw luma range `0..255` and mean 44.
The original full-active-area grid compared those values directly and reported
45 permille, above its 8-permille threshold.

The following native 30-second movie decoded 749 frames. Its luma mean stayed
within `53..54`, strong-row statistic within `69..73` permille, and both
previous-frame and one-second temporal deltas remained exactly zero. Attempt 45
therefore supplies no evidence of FPGA corruption and no justification for a
post-OSD observer.

The corrected detector keeps black and capture-card signal-loss classification
unchanged. Before the temporal grid is accumulated, a frame whose sampled luma
endpoints are full range (`minimum <= 1`, `maximum >= 254`) is converted to the
canonical video range using rounded `16 + value * 219 / 255`. Already
video-range frames are unchanged. Temporal comparison then uses only the left
eight of the existing sixteen grid columns across all nine rows. That region
contains the settled Arcade list and excludes the animated preview.

The evidence contract is `8x9-static-left-video-range-v2`; old grid identities
fail closed. The threshold is 2 permille. Native AVFoundation calibration found:

- 2,186 one-second comparisons from three known-good static launcher movies:
  exactly 0 permille;
- all 708 one-second comparisons from the preserved genuine moving-corruption
  movie: `2..657` permille in the static-left region;
- all 749 frames in the post-attempt-45 movie: stable and healthy;
- synthetic full-range/video-range equivalents: 0 permille;
- synthetic preview-only animation: 0 permille;
- synthetic full-width moving band: at or above the corruption threshold;
- black and signal-loss fixtures: unchanged fail-closed classification.

This change repairs only physical-evidence classification. It does not change
USB capture, the installed RBF, Main, the launcher, latch-v5, or any FPGA logic.
