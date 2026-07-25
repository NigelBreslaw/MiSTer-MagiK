# Benchmarking policy

`scripts/agent benchmark` is the only agent-facing performance workflow. It is
flag-free and profiles the screensaver inside the installed development app.
It never builds, deploys, replaces platform files, or reboots the MiSTer. The
installed platform manifest and its hashes are the benchmark identity,
regardless of the local Git HEAD.

## Installed screensaver profile

The fixed workflow is:

```text
Verify installed platform and health
-> require screensaver-pprof-v1 and a cached catalog
-> transactionally select 1280x720 HDMI and verify a 1280x720 RGB565 framebuffer
-> start the ordinary launcher with a one-shot environment
-> navigate Home -> Settings -> Show Screensaver
-> profile and stream telemetry for 30 seconds
-> restore the ordinary launcher
-> repeat in a fresh launcher process
-> restore the original display mode and exact MiSTer.ini contents
-> verify platform identity and health
```

The benchmark temporarily confirms `hdmi-1280x720p60` so the mode remains
active for both 30-second profiles. It records the original mode and original
`MiSTer.ini`, then restores the mode through Main's typed display transaction.
Byte-for-byte INI restoration is mandatory. Display restoration runs after
every outcome and is independent of launcher/profile cleanup.

The one-shot launcher environment sets `MISTER_CATALOG_REFRESH=off`. The
launcher must report both the disabled refresh policy and an inactive catalog
worker throughout each active profile. The environment removes itself when the
launcher consumes it; bounded host cleanup is still unconditional after
success, failure, interruption, or an incomplete second run.

Each run uses the production composition and latch-backed presentation path.
Navigation frames are excluded. The first three frames for which runtime
telemetry reports `screensaver_active=true` are activation warm-up: their
timings are recorded as startup evidence but never fail the benchmark. A
screensaver may take several frames to become visible without creating a
user-visible defect.

Steady state begins on the fourth active screensaver frame. Every steady-state
frame must finish within the measured refresh period; a single over-budget
frame fails the run. Average steady-state FPS must be at least 55 so a stalled
or incomplete capture cannot pass. P99 and maximum timings remain evidence,
not independent gates. Presentation errors, latch drops, and steady-state
vsync misses must remain zero.

This distinction is intentional. Do not tighten startup timing because of a
slow first render, asset loading, allocation, profiler startup, or other
one-time activation work. The benchmark exists to prove that an already
running screensaver does not drop frames.

## Restoration

Restoration removes the launcher environment, frame-analytics lease, and
temporary remote profile files, then restarts the ordinary launcher. The
workflow fails separately when performance is outside its gates or restoration
cannot prove a clean, healthy device. The original display mode and exact
`MiSTer.ini` contents, device boot ID, and installed manifest must be unchanged.

## Evidence

Both SVG flamegraphs, folded stacks, profile metadata, telemetry streams, and a
`summary.json` are written under
`build/agent-benchmarks/screensaver/<timestamp>/`. Evidence records the
installed revision, display route and framebuffer geometry, and GUI, Main,
scanout-module, and latch-RBF hashes. Pure offline report generators under
`scripts/bench/` may analyze existing data; they must not contact or mutate a
MiSTer.
