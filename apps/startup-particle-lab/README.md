<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Startup particle lab

This is the focused, Slint-free development app for the production-quality
MagiK text and arcade-cabinet particle effects. The recipe schema selects the
effect; there is deliberately no demo registry or recipe-family abstraction.
The separate 36-demo `apps/framebuffer-lab` remains unchanged and is not a
dependency of this app.

Use the supported host workflow for a macOS preview:

```text
scripts/agent startup-particles preview RECIPE
```

For attended MiSTer sessions, use one of:

```text
scripts/agent device startup-particles RECIPE --runtime lab --attended
scripts/agent device startup-particles RECIPE --runtime dev-launcher --attended
```

The focused lab accepts the MagiK and cabinet schemas. The Dev launcher accepts
only MagiK and watches the fixed volatile
`/tmp/mister-magik/startup-particles/magik.json` path. The public launcher never
watches an external recipe.

Before an attended lab suspends the launcher, the host reads Main's confirmed
fixed HDMI mode and passes that exact destination to the standalone latch
presenter. The RGB565 source remains 960x540; at 1080p the FPGA scales it to a
1920x1080 destination. The lab never infers HDMI geometry from core-video
registers after Main has been suspended. CRT routes remain with the production
launcher because their direct-video porch offsets are route-specific.

Run a live macOS preview with a validated recipe:

```text
mister-magik-startup-particle-lab --recipe path/to/magik.json
```

Saving the recipe reloads the renderer. The last valid renderer stays visible
after rejected or partial saves, and `status.json` is written beside the recipe.
The watcher polls every 100 ms, accepts at most 1 MiB, retains only the newest
generation, and restores the embedded recipe once after two missing polls.

Create a deterministic capture without opening a window:

```text
mister-magik-startup-particle-lab \
  --recipe path/to/cabinet.json \
  --time-ms 5000 \
  --output cabinet.ppm
```

The engine, palettes, frame hashes, and MiSTer presentation remain RGB565. The
macOS preview expands the completed RGB565 frame only for its XRGB8888 window
surface, and PPM output expands it only because PPM stores three eight-bit color
channels. Neither adapter is an RGB888 simulation or framebuffer path.

The recipe schemas, status protocol, asset authority, and compile boundary are
documented in `docs/startup-particles.md`.
