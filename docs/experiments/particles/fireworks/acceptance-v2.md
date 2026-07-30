# Fireworks V2 commercial acceptance

The six V2 fireworks are complete only when both independent visual reviewers
return `SHIP` for every final FPGA-latched capture. Embedded constraints do not
lower the artistic bar.

## V1 comparison baseline

The existing six `mister-magik-firework-v1` presets, their renderer behavior,
hero hashes, showcase positions 1–6, and committed framebuffer evidence remain
available as the comparison baseline. V2 work must not overwrite or silently
retune those JSON files.

The six new V2 presets use distinct IDs and showcase positions 7–12 in the same
style order. Capture tooling must be able to select either version by ID so each
concept can be compared against V1 and V2 at the same hero time and seed.

## Ship gate

Each reviewer evaluates the six full-resolution 960×540 RGB565 captures against
the committed concept images and reports:

- concept fidelity out of 10;
- standalone commercial quality out of 10;
- `SHIP`, `REFINE`, or `REDESIGN`;
- the most damaging remaining gap.

Acceptance requires:

- `SHIP` from both reviewers for all six styles;
- no unresolved silhouette, motion, material, hierarchy, palette, clipping, or
  staging defect;
- the first active device renderer is `particle-demos`;
- no device telemetry frame observes `particle-magik`;
- every capture comes from `fpga-latched-scanout-slots`;
- the normal launcher is restored after each capture.

Scores are diagnostic evidence, not a substitute for the explicit `SHIP`
verdict.

## Shared visual requirements

1. Motion follows integrated trajectories or authored curves. Rotating a scalar
   radius is not accepted as ballistic, curling, or vortex motion.
2. Trails expose a tapered luminous core, soft colored falloff, persistent hot
   heads, width-over-life, and controlled bead or fragment breakup. Uniform hard
   rods are rejected.
3. Parent paths can emit delayed children that inherit position and velocity.
   Terminal glitter, willow fragmentation, chrysanthemum branches, and recursive
   satellites must be staged rather than simulated with unrelated simultaneous
   bursts.
4. Spherical fireworks use depth-aware shell sampling and projection rather
   than a flat circle.
5. Brightness, width, birth timing, color assignment, and decay are authored
   declaratively per layer. Additive accumulation must preserve palette identity
   in RGB565.
6. Hero silhouettes retain deliberate black margins and readable negative space
   at 960×540.

## Style-specific requirements

### Solar Chrysanthemum

- A white-hot ignition and compact red inner core anchor the composition.
- A limited number of coherent gold branches crest and bend downward under
  gravity, shedding smaller fragments along their paths.
- The cyan shock shell is delicate, non-uniform, short-lived, and subordinate.

### Recursive Halo

- Three luminous rings have distinct material, timing, and brightness.
- Eight child rosettes are synchronized descendants of the halo and do not
  collide with the parent rings or frame edge.
- Falling gold detail remains secondary and preserves black separation.

### Copper Willow Rain

- A sparse crown feeds 24–40 long copper ropes that crest, decelerate, droop,
  fragment, and retain bright descending heads.
- Copper and ember red dominate; olive is a quiet rear accent.
- The silhouette never becomes a fan, lampshade, or solid triangular curtain.

### Phoenix Comet

- Tail, body, head, and asymmetric wings form one tangent-continuous gesture.
- Feather streams follow authored curves and do not form radial wedges,
  cross-hatching, or a central black cavity.
- Turquoise appears at selected terminal tips rather than as uniformly mixed
  spokes.

### Magnetic Flower

- Six open counter-rotating petals remain individually readable.
- Curvature, length, density, and timing vary enough to avoid a mechanical
  turbine.
- Green orbits and gold satellites are accents, not hard enclosing borders.

### OLED Peony

- A volumetric magenta/red inner shell sits inside a cobalt/cyan outer shell.
- Sparse gold terminal heads and a white-hot core remain independently legible.
- Trails are fine, luminous, gravity-bent, and dense without flattening into a
  uniform multicolor spoke field.

## Review cadence

Use deterministic local RGB565 captures for rapid experiments. Request the two
independent reviewers only after a materially new engine or art milestone.
Consolidate their rejected criteria into one coherent iteration, then recapture.
Deliver to the MiSTer only after both reviewers accept the local milestone.
