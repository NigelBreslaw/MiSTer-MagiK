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
scripts/agent device scene-lab --scene card-flip --assess --attended
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

Use the closed attended assessment to run an authoritative 30-second unprofiled
cadence pass followed by a separate 30-second 99 Hz CPU-sampled attribution
pass. Both passes execute the same optimized `release-device` binary, and the
launcher is restored on every exit path:

```text
scripts/agent device scene-lab --scene card-flip --assess --attended
```

The older single sampled pass remains available when only an interactive CPU
profile is wanted:

```text
scripts/agent device scene-lab --scene card-flip --profile --attended
```

The timing fields are deliberately disjoint. `render` covers projection and
rasterization, `transfer` covers only `prepare_cached`, `present` runs from the
latch post through confirmed presentation, and `frame_to_present` runs from
render start through that same confirmation. `cpu_pct` is whole-process CPU
time divided by wall time. Physical cadence is calculated only from monotonic
confirmation intervals and wrapping latch flip-counter deltas. A
`repeated_refreshes` result from the unprofiled pass is the skip authority;
sequence failures, latch drops, and completion failures are separate results.
The sampled pass can attribute a failure but can never qualify cadence.

Two consecutive 2026-08-05 assessments of commit `0e335037`, using the same
optimized ARMv7 binary hash in all four passes, produced:

| Run | Pass | Physical FPS | CPU | Render avg / p99 | Transfer avg / p99 | Post avg / p99 | Settle avg / p99 | Post-to-confirm avg / p99 | Frame-to-confirm avg / p99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | Unprofiled authority | 59.950 | 40.72% | 3.770 / 6.221 ms | 0.601 / 0.718 ms | 0.089 / 0.145 ms | 10.057 / 13.571 ms | 12.156 / 15.664 ms | 16.645 / 16.824 ms |
| 1 | 99 Hz attribution | 59.950 | 44.04% | 4.101 / 6.415 ms | 0.661 / 0.822 ms | 0.099 / 0.158 ms | 9.667 / 13.391 ms | 11.761 / 15.489 ms | 16.643 / 16.813 ms |
| 2 | Unprofiled authority | 59.949 | 41.58% | 3.873 / 6.256 ms | 0.620 / 0.741 ms | 0.092 / 0.178 ms | 9.929 / 13.513 ms | 12.028 / 15.621 ms | 16.643 / 16.811 ms |
| 2 | 99 Hz attribution | 59.950 | 43.40% | 4.042 / 6.411 ms | 0.649 / 0.811 ms | 0.095 / 0.156 ms | 9.735 / 13.411 ms | 11.834 / 15.501 ms | 16.641 / 16.816 ms |

Each unprofiled authority recorded 1,800 confirmed frames, 1,799 expected
refresh intervals, and 1,799 unique latch flips. Both had zero physical repeated
refreshes, zero sequence failures, zero latch drops, zero completion failures,
and no long confirmation intervals. The largest confirmation gaps were 16.878
ms and 16.898 ms, still one 60 Hz refresh interval. Consequently, the earlier
59.5 FPS aggregate was not evidence of physical frame skipping.

The two sampled runs collected 1,428 and 1,328 stack samples. Respectively, 552
and 535 resolved into `CardFlip::paint_rows_device`, while 605 and 513 resolved
into the shared latch settle path, including expected sleeping and status
polling. Only 3 and 6 samples reached `prepare_cached`, and 10 and 8 reached
`post_prepared`. The matched render wall/CPU time identifies the rasterizer as
the real active CPU cost; the much larger settle wall time than settle CPU time
is expected vblank waiting rather than an over-budget renderer stall.

Every steady frame remains one 287x420 rectangle: 241,080 source bytes and
241,080 destination bytes. The optimized device paint symbol contains no calls,
software or hardware division, wide multiply, allocation, or bounds-check panic
paths. Neither `scale_card_frame` nor `scaled_card_rect` exists in source or the
ARM profile binary.

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
