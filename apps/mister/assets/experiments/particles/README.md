# Particle recipe families

These JSON recipe families belong to particle experiments compiled only with
the explicit `experiments` feature. Production builds do not embed them.

The editable families are deliberately small and closed:

- `fireworks.json` contains showcase demos 1–12;
- `procedural.json` contains demos 13–31;
- `form.json` contains demos 32–36.

Each file is a complete family. Unknown fields, missing demos, invalid colors,
non-finite numbers, and values outside the renderer's fixed capacities are
rejected. Rendering algorithms, framebuffer/latch behavior, SIMD code, and
other engine invariants remain in Rust.

## Live editing on macOS

Start the compiled RGB565 showcase once, selecting a family and a demo from
that family:

```bash
apps/mister/scripts/dev-ui-mac.sh \
  --scenario particle-showcase \
  --particle-family apps/mister/assets/experiments/particles/procedural.json \
  --particle-demo 13
```

Saving the selected JSON file reloads it without recompiling. A valid save
restarts the selected effect at time zero. An incomplete or invalid save leaves
the last valid recipe running; correct the file and save again.

For a deterministic one-frame artifact, the particle preview binary accepts
the same family contract without starting a watch session:

```bash
mister-magik-particle-preview \
  --family apps/mister/assets/experiments/particles/procedural.json \
  --demo 13 \
  --time-ms 15000 \
  --hud off \
  --output /tmp/particle-13.ppm
```

## Live editing on MiSTer

Install the explicit experimental development runtime from a clean commit:

```bash
scripts/agent live-particles install
```

Then start one attended watch session:

```bash
mister live particles \
  apps/mister/assets/experiments/particles/procedural.json \
  --demo 13 \
  --attended
```

The host validates and atomically publishes each distinct save. The device
performs the full semantic validation, retains the last good family after a
rejection, and reports the applied or rejected generation. `Ctrl-C` removes the
volatile recipe/status files and returns Main's supervised development launcher
to its normal configuration. The experimental binary remains the development
runtime until an ordinary `scripts/agent deliver` replaces it; production is
never modified by this workflow.

The retained arcade-cabinet model and compiled point cloud intentionally remain
in `apps/mister/assets/particles/` because the arcade formation is reserved for
the future startup animation.
