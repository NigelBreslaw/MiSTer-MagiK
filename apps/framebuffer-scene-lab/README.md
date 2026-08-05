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
display state and resolves it through the same MagiK display-plan authority.
That plan supplies the render, framebuffer, scan, and output geometry to the
standalone latch presenter. For a 1920x1200 output the canonical RGB565 render
surface is 960x600; there is no lab-local 960x540 adapter or scaling pass. The
lab never infers HDMI geometry from core-video registers after Main has been
suspended. CRT routes retain the shared production vertical transform because
their direct-video porch offsets are route-specific.

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
MiSTer uses a separate row-major fixed-point rasterizer that draws directly into
the display plan's cached RGB565 render surface. The shared hidden-latch
presenter copies the resolved card damage rectangle into its two remembered
slots; only each slot's first use restores the full surface.
A/Enter flips forward and B/Backspace flips backward on macOS; the MiSTer
controller uses A and B. Each face repeats the same left-to-right door-hinge
trajectory, so continuous animation keeps the same perceived rotation direction
instead of bouncing through the previous path. Face endpoints are identical
across the trajectory reset, and direction changes continue from the current
progress.

Use the closed attended profile to alternate directions continuously for 30 seconds and
restore the launcher automatically:

```text
scripts/agent device scene-lab --scene card-flip --profile --attended
```

The timing fields are deliberately disjoint. `render` covers projection and
rasterization, `transfer` covers only `prepare_cached`, `present` runs from the
latch post through confirmed presentation, and `frame_to_present` runs from
render start through that same confirmation. `cpu_pct` is whole-process CPU
time divided by wall time.

Two consecutive 2026-08-05 continuous ARMv7 profiles of commit `e82859c8`
produced:

- Run 1: 59.510 fps and 41.00% CPU; render 3.790 ms average / 6.343 ms p99,
  transfer 0.599 / 0.783 ms, present 12.252 / 15.716 ms, and
  frame-to-present 16.650 / 16.824 ms.
- Run 2: 59.513 fps and 41.62% CPU; render 3.856 ms average / 6.335 ms p99,
  transfer 0.619 / 0.822 ms, present 12.163 / 15.710 ms, and
  frame-to-present 16.648 / 16.821 ms.

Both runs presented 1,787 changed frames with zero repeats and zero latch drops.
Each reported exactly two full-slot restores; every steady frame was one
287x420 rectangle (241,080 source bytes and 241,080 destination bytes), for
432,631,800 bytes per side over the run. The optimized device paint symbol
contains no calls, software or hardware division, wide multiply, allocation, or
bounds-check panic paths. Neither `scale_card_frame` nor `scaled_card_rect`
exists in source or the ARM profile binary.

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
