# Firework visual contract

The six concept images and `manifest.json` in this directory are the visual
authority for the first MiSTer MagiK firework set. The initial acceptance target
is an excellent still and convincing motion at native 960×540 RGB565. Performance
work starts only after the visuals are accepted.

## Canonical hero frames

All captures use seed `827141709451`, a black background, and no HUD.

| Firework | Hero time | Exact local RGB565 frame hash |
| --- | ---: | --- |
| Solar Chrysanthemum | 2100 ms | `fc0c900a27f8b67e` |
| Recursive Halo | 2200 ms | `e439f88f56eed5d6` |
| Copper Willow Rain | 2500 ms | `a4054c9e752e5718` |
| Phoenix Comet | 2350 ms | `f8084f2140ddf0da` |
| Magnetic Flower | 2500 ms | `9d81df4004e0683b` |
| OLED Peony | 2000 ms | `e18c762ea9aa270a` |

The hashes are renderer-reported PPM pixel hashes, not PNG file hashes. They
detect accidental output drift; visual comparison against the concept remains
the artistic acceptance test.

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

## Fast iteration

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

The device workflow must observe `particle-demos` as the first active renderer,
reject any observation of `particle-magik`, capture the authoritative
framebuffer, and restore the normal launcher even if capture fails. The full
spinning MagiK particle demo is never an initialization step for firework work.

Use two visual-review milestones: one local review while the primitives are
fluid, then one real-framebuffer review after delivery. Combine actionable
feedback into at most one refinement per milestone instead of repeatedly
reviewing nearly identical frames.

[niagara-ribbons]: https://dev.epicgames.com/documentation/en-us/unreal-engine/how-to-create-a-ribbon-effect-in-niagara-for-unreal-engine
[unity-strips]: https://activation.unity3d.com/releases/2019-3/graphics
[houdini-explosion-trails]: https://www.sidefx.com/docs/houdini/pyro/explode.html
[houdini-sparks]: https://www.sidefx.com/docs/houdini/pyro/sparks.html
