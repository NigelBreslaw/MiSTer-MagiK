<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Framebuffer scene lab

This is the focused, Slint-free development app for portable production RGB565
scenes: the MagiK text and arcade-cabinet particle effects plus launcher
navigation-transition rasterization. The command line selects one concrete
scene; there is deliberately no demo registry or recipe-family abstraction.
The separate 36-demo `apps/framebuffer-lab` remains unchanged and is not a
dependency of this app.

Use the supported host workflow for a macOS preview:

```text
scripts/agent scene-lab preview --scene magik --recipe RECIPE
scripts/agent scene-lab preview --scene cabinet --recipe RECIPE
scripts/agent scene-lab preview --scene navigation-transition --fixture home-arcade
```

For attended MiSTer sessions, use one of:

```text
scripts/agent device scene-lab --scene magik --recipe RECIPE --attended
scripts/agent device scene-lab --scene navigation-transition --fixture home-arcade --attended
scripts/agent device startup-particles RECIPE --runtime dev-launcher --attended
```

The interactive cabinet lab uses left/right for colour and creative modes,
up/down for exact 1,024-particle count steps, and `A` to toggle the particle
targets between the original surface cloud and equally spaced MRI slices.
macOS arrow keys and the `A` key mirror those controls without key repeat.

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

The engine, palettes, frame hashes, and MiSTer presentation remain RGB565. The
macOS preview expands the completed RGB565 frame only for its XRGB8888 window
surface, and PPM output expands it only because PPM stores three eight-bit color
channels. Neither adapter is an RGB888 simulation or framebuffer path.

The recipe schemas, status protocol, asset authority, and compile boundary are
documented in `docs/startup-particles.md`.
