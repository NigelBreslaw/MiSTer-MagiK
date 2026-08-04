<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Framebuffer scene lab

This is the focused, Slint-free development app for portable production RGB565
scenes: the MagiK text and arcade-cabinet particle effects, launcher
navigation-transition rasterization, and the procedural card flip. The command
line selects one concrete scene; there is deliberately no demo registry or
recipe-family abstraction.
The separate 36-demo `apps/framebuffer-lab` remains unchanged and is not a
dependency of this app.

Use the supported host workflow for a macOS preview:

```text
scripts/agent scene-lab preview --scene magik --recipe RECIPE
scripts/agent scene-lab preview --scene cabinet --recipe RECIPE
scripts/agent scene-lab preview --scene navigation-transition --fixture home-arcade
scripts/agent scene-lab preview --scene card-flip
```

For attended MiSTer sessions, use one of:

```text
scripts/agent device scene-lab --scene magik --recipe RECIPE --attended
scripts/agent device scene-lab --scene navigation-transition --fixture home-arcade --attended
scripts/agent device scene-lab --scene card-flip --attended
scripts/agent device startup-particles RECIPE --runtime dev-launcher --attended
```

The older `startup-particles preview RECIPE` and attended `--runtime lab`
commands remain typed compatibility aliases. The old particle-only lab app and
binary names are compatibility-only; new workflows use `framebuffer-scene-lab`.
The focused lab accepts the MagiK and cabinet schemas or one of the generated
`home-arcade`, `home-consoles`, and `consoles-system` navigation fixtures. The
Dev launcher accepts
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
mister-magik-framebuffer-scene-lab --scene magik --recipe path/to/magik.json
```

Saving the recipe reloads the renderer. The last valid renderer stays visible
after rejected or partial saves, and `status.json` is written beside the recipe.
The watcher polls every 100 ms, accepts at most 1 MiB, retains only the newest
generation, and restores the embedded recipe once after two missing polls.

Navigation fixtures contain generated RGB565 cards and backgrounds, require no
Slint or catalog, and cycle through forward and reverse directions. They are
immutable; `--fixture` and `--recipe` are mutually exclusive.

The card flip is self-contained and accepts neither option. Its 258x378 faces
are drawn in memory from rectangles and 5x7 bitmap glyphs; there are no card
assets or regeneration step. The macOS preview is a readable scalar reference.
MiSTer uses a separate row-major fixed-point rasterizer, renders into a cached
RGB565 frame, and transfers changed frames to scanout with an ARMv7 NEON copy.
A/Enter flips forward and B/Backspace flips backward on macOS; the MiSTer
controller uses A and B. Direction changes continue from the current pose.

Use the closed attended profile to alternate directions continuously for 30 seconds and
restore the launcher automatically:

```text
scripts/agent device scene-lab --scene card-flip --profile --attended
```

The 2026-08-04 ARMv7 profile measured 3.372 ms average / 5.312 ms p99 render,
1.999 ms average / 2.339 ms p99 transfer, zero repeated presentations, and zero
latch drops over 507 changed frames. The isolated device paint symbol contains
no calls, software division, wide multiply, or bounds-check branches in its
generated release assembly.

Create a deterministic capture without opening a window:

```text
mister-magik-framebuffer-scene-lab \
  --scene cabinet \
  --recipe path/to/cabinet.json \
  --time-ms 5000 \
  --output cabinet.ppm
```

The same capture contract applies to navigation fixtures:

```text
mister-magik-framebuffer-scene-lab \
  --scene navigation-transition \
  --fixture consoles-system \
  --time-ms 1080 \
  --output transition.ppm
```

Card checkpoints are recipe-free and can select a direction:

```text
scripts/agent scene-lab capture \
  --scene card-flip \
  --direction forward \
  --time-ms 220 \
  --output card-midpoint.ppm
```

The engine, palettes, frame hashes, and MiSTer presentation remain RGB565. The
macOS preview expands the completed RGB565 frame only for its XRGB8888 window
surface, and PPM output expands it only because PPM stores three eight-bit color
channels. Neither adapter is an RGB888 simulation or framebuffer path.

The recipe schemas, status protocol, asset authority, and compile boundary are
documented in `docs/startup-particles.md`.
