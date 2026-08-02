# MiSTer MagiK Framebuffer Lab

This package is a Slint-free RGB565 particle sandbox. It contains the 36-demo
experimental showcase, validated JSON recipe families, deterministic capture,
and a native macOS `winit`/`softbuffer` preview.

Interactive macOS preview:

```bash
scripts/agent live-particles preview \
  apps/framebuffer-lab/assets/experiments/particles/procedural.json \
  --demo 13
```

The selected JSON file is polled every 100 ms. A valid save restarts the pinned
demo at logical time zero. Invalid content leaves the last good recipe active
and reports the bounded error in the window title. Escape closes the window.
The same command is the Rust iteration loop: stop it, edit the standalone lab,
and rerun without compiling the Slint application.

Attended MiSTer preview uses the same renderer and recipe watcher while writing
straight to the hidden RGB565 scanout slots and presenting through the latch:

```bash
scripts/agent device live-particles \
  apps/framebuffer-lab/assets/experiments/particles/procedural.json \
  --demo 13 --attended
```

The device session uploads only the lab binary and selected recipe to a volatile
directory. Saving valid JSON updates the running session; invalid JSON keeps the
last good recipe. Ctrl-C restores the launcher and removes the session files.

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
