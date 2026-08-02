# MiSTer MagiK Framebuffer Lab

This package is a Slint-free RGB565 particle sandbox. It contains the 36-demo
experimental showcase, validated JSON recipe families, deterministic capture,
and a native macOS `winit`/`softbuffer` preview.

Interactive macOS preview:

```bash
cargo run --manifest-path apps/framebuffer-lab/Cargo.toml -- \
  --demo 13 \
  --family apps/framebuffer-lab/assets/experiments/particles/procedural.json
```

The selected JSON file is polled every 100 ms. A valid save restarts the pinned
demo at logical time zero. Invalid content leaves the last good recipe active
and reports the bounded error in the window title. Escape closes the window.

Deterministic headless capture parses the family exactly once and starts no
watcher:

```bash
cargo run --manifest-path apps/framebuffer-lab/Cargo.toml -- \
  --demo grid-flocking \
  --family apps/framebuffer-lab/assets/experiments/particles/procedural.json \
  --time-ms 15000 \
  --output /tmp/grid-flocking.ppm
```

Validate a demo and recipe before opening a display or framebuffer:

```bash
cargo run --manifest-path apps/framebuffer-lab/Cargo.toml -- \
  --demo 13 \
  --family apps/framebuffer-lab/assets/experiments/particles/procedural.json \
  --check
```

The package intentionally does not depend on Slint, generated UI, FFmpeg,
catalog/media crates, or Swash. Rust engine changes require restarting the lab;
automatic Rust source watching is out of scope.
