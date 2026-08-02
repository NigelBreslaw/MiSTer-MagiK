# Particle Lab Compile-Time Comparison

Measured on 2026-08-02 with the v2 harness: one cold build, five unchanged
invocations, one particle-source warm-up, and five forced particle-source
rebuilds. Every forced sample changed the source bytes uniquely, and the exact
source bytes and timestamps were restored afterward.

| Target | Cold before | Cold after | Rebuild before | Rebuild after | Rebuild speedup |
| --- | ---: | ---: | ---: | ---: | ---: |
| macOS | 75.328 s | 8.685 s | 2.680 s | 0.826 s | 3.24x |
| ARM | 139.863 s | 13.802 s | 26.722 s | 5.862 s | 4.56x |

The before target was the retired full Slint application with the experimental
showcase enabled. The after target is `apps/framebuffer-lab`, which has no Slint,
generated UI, FFmpeg, catalog/media, or Swash dependency. ARM times include the
typed Apple-container workflow overhead; Cargo's in-container particle compile
was about 4.2–4.5 seconds per sample.

The adjacent `*-v2-20260802.json` reports retain every sample and the source
hashes. The earlier v1 before reports are superseded because future mtimes did
not prove that Cargo rebuilt every forced sample.
