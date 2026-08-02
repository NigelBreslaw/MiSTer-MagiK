# Archived firework visual contract

This is the historical visual contract for the retired firework experiment.
The six large concept PNGs are no longer version-controlled. Their metadata
remains in `manifest.json`; existing local copies live under the ignored
`build/particle-experiments/fireworks/` directory.

## Canonical hero frames

All captures use seed `827141709451`, a black background, and no HUD.

| Firework | Hero time | Exact local RGB565 frame hash |
| --- | ---: | --- |
| Solar Chrysanthemum | 2100 ms | `40b77ebdd3e5a9df` |
| Recursive Halo | 2200 ms | `df8ace68654fb5c8` |
| Copper Willow Rain | 2500 ms | `8c92a48429162f81` |
| Phoenix Comet | 2350 ms | `1e84e839507fba14` |
| Magnetic Flower | 2500 ms | `029968672f2cba94` |
| OLED Peony | 2000 ms | `5cab1a90691132e0` |

The hashes are renderer-reported PPM pixel hashes, not PNG file hashes. They
detect accidental output drift; visual comparison against the concept remains
the artistic acceptance test. The macOS preview and device workflow share the
same seed. Small edge-pixel differences from x86_64 versus ARM floating-point
evaluation are expected; the FPGA-latched device PNG is authoritative.

## Declarative vocabulary

Each strict `mister-magik-firework-v1` JSON file composes a small set of reusable
primitives:

- timed and repeating emitters;
- radial, ring, fan, spiral, arc, comet, and burst shapes;
- bounded particle count, lifetime, speed, gravity, drag, and origin;
- continuous additive trail strokes with length, spacing, and fade;
- strand families that group nearby trajectories without precomputing paths;
- emitter rotation, angular velocity, and bounded per-particle curl;
- independent palettes, brush intensity, decay envelopes, and twinkle.

Unknown fields and unsafe bounds fail at load time. Evaluation is deterministic
from absolute show time, so a frame can be reproduced without simulating every
earlier frame.

This deliberately small system reflects techniques used by established particle
engines: [Niagara ribbons][niagara-ribbons] and [Unity particle
strips][unity-strips] for continuous luminous trails, plus Houdini's
[ballistic trail path shapes][houdini-explosion-trails] and [split particle
trails][houdini-sparks] for gravity-dominated and coherent branches. It is not a
general node graph.

## Historical iteration notes

The commands below are retained as evidence of the former workflow. Their
production and standard-preview entry points have been removed.

After building the macOS UI preview through the repository workflow, capture an
exact local frame with:

```text
mister-magik-ui-preview --scenario fireworks --firework oled-peony \
  --time-ms 2000 --hud off --output /tmp/oled-peony.ppm
```

Capture the installed RGB565 framebuffer through the typed device workflow:

```text
scripts/agent benchmark firework-visual --firework oled-peony
scripts/agent benchmark firework-visual --all
```

Those retired captures required `particle-demos` as the first active renderer.
Current iteration uses the standalone framebuffer lab and its volatile attended
device session; it does not launch the full Slint application.

Use two visual-review milestones: one local review while the primitives are
fluid, then one real-framebuffer review after delivery. Combine actionable
feedback into at most one refinement per milestone instead of repeatedly
reviewing nearly identical frames.

[niagara-ribbons]: https://dev.epicgames.com/documentation/en-us/unreal-engine/how-to-create-a-ribbon-effect-in-niagara-for-unreal-engine
[unity-strips]: https://activation.unity3d.com/releases/2019-3/graphics
[houdini-explosion-trails]: https://www.sidefx.com/docs/houdini/pyro/explode.html
[houdini-sparks]: https://www.sidefx.com/docs/houdini/pyro/sparks.html
