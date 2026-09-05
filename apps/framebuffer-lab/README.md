# MiSTer MagiK Framebuffer Lab

The legacy agent preview, capture, device-session and lab measurement commands
were removed in milestone 4. This document retains the underlying renderer
and asset contracts; it is not an instruction to recreate those workflows.


This package is a Slint-free RGB565 particle sandbox. It contains the 36-demo
experimental showcase, validated JSON recipe families, deterministic capture,
and a native macOS `winit`/`softbuffer` preview.

The selected JSON file is polled every 100 ms. A valid save restarts the pinned
demo at logical time zero. Invalid content leaves the last good recipe active
and reports the bounded error in the window title. Escape closes the window.

The former device-session uploader and launcher-restoration workflow have been
deleted. The following describes retained native binary capabilities, not a
supported host development workflow.

Deterministic headless capture parses the family exactly once and starts no
watcher:

```text
mister-magik-particle-lab --demo grid-flocking \
  --family apps/framebuffer-lab/assets/experiments/particles/procedural.json \
  --time-ms 15000 --output /tmp/grid-flocking.ppm
```

Validate a demo and recipe before opening a display or framebuffer:

```text
mister-magik-particle-lab --demo 13 \
  --family apps/framebuffer-lab/assets/experiments/particles/procedural.json \
  --check
```

The package intentionally does not depend on Slint, generated UI, FFmpeg,
catalog/media crates, or Swash. Rust engine changes require restarting the lab;
automatic Rust source watching is out of scope.
