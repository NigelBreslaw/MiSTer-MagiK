# Particle technique commercial acceptance

The ten particle techniques are complete only when two independent visual
reviewers return `SHIP` for every final FPGA-latched capture. The reference
PNGs define composition, hierarchy, material, palette, and negative-space
targets at 960x540 RGB565; they are not pixel-matching targets.

## Deterministic contract

- seed: `827141709451`
- framebuffer: 960x540 direct RGB565
- background: true black
- HUD: hidden for review captures
- duration: 30 seconds per technique
- stills: entry, hero, and exit beats
- motion: one continuous 5-8 second USB Video capture containing the hero beat
- authority: `fpga-latched-scanout-slots`; `/dev/fb0` evidence is invalid

| # | Technique | Budget | Entry | Hero | Exit |
|---:|---|---:|---:|---:|---:|
| 22 | Procedural Sprite Materials | 16,384 | 2 s | 12 s | 26 s |
| 23 | Variable-width Ribbons | 8,192 | 2 s | 14 s | 27 s |
| 24 | Curl-noise Flow Field | 32,768 | 3 s | 15 s | 27 s |
| 25 | Low-resolution Density Bloom | 24,576 | 3 s | 16 s | 27 s |
| 26 | Layered Child Systems | 4,096 | 2 s | 12 s | 24 s |
| 27 | Spatial Field Stack | 24,576 | 3 s | 15 s | 27 s |
| 28 | Depth-aware Material LOD | 40,960 | 3 s | 16 s | 27 s |
| 29 | Source-driven Morph | 12,288 | 4 s | 10 s | 19 s |
| 30 | SDF Collision Events | 8,192 | 3 s | 15 s | 27 s |
| 31 | Grid-accelerated Flocking | 12,288 | 3 s | 15 s | 27 s |

The budget is the fixed active-particle or active-agent ceiling. Ribbons also
require 24 continuous hero ribbons and 192 cheaper streaklets.

## Shared ship gate

Each reviewer scores composition, material quality, motion coherence, depth,
RGB565 colour handling, and artifact freedom from 1-5, then returns `SHIP`,
`REFINE`, or `REDESIGN`.

Acceptance requires:

- `SHIP` from both reviewers;
- every score at least 4;
- an intentional silhouette with readable black margins at native 1x scale;
- no stale pixels, flicker, popping, clipping, visible intermediate buffers,
  saturation collapse, or allocation-related cadence changes;
- no loss of a defining visual read after performance tuning;
- physical presentation within 0.1 FPS of 60 Hz, render P99 below the refresh
  period minus 750 microseconds, and no dropped frames, misses, latch drops,
  starvation, reused frames, or superseded frames.

## Technique-specific requirements

1. **Procedural Sprite Materials:** spark, four-point star, glow disc, smoke
   puff, and shard remain distinguishable at 1x. White-hot cores, warm
   over-life colour, quantized soft edges, and clean fades appear together.
2. **Variable-width Ribbons:** a continuous tapered S-gesture has stable joins,
   bright moving heads, separated cyan/cobalt/violet/magenta layers, sparse
   gold accents, and restrained bead breakup.
3. **Curl-noise Flow Field:** two curls and a counter-current coexist. Tracers
   expose broad persistent flow and evolving eddies without jitter, lattice,
   synchronized motion, popping, or teleporting.
4. **Low-resolution Density Bloom:** a crescent retains black cavities,
   individual edge particles, bright ridges, and at least four stepped
   luminance bands without a visible buffer boundary or white interior.
5. **Layered Child Systems:** three parents remain readable through
   speed-dependent shedding, one cyan event ring per parent, and delayed
   terminal children. Counts remain bounded without a full-screen flash.
6. **Spatial Field Stack:** attraction, repulsion, and
   capture-orbit-release occupy distinct regions. The repulsor keeps a clean
   cavity, and particle motion alone explains every force.
7. **Depth-aware Material LOD:** at least three depth layers differ through
   size, softness, brightness, saturation, streak length, and parallax.
   Transitions do not pop and the central travel corridor remains open.
8. **Source-driven Morph:** joystick and controller endpoints are recognizable
   without labels, remain stable at their holds, and morph through ordered,
   sparse assignment arcs without becoming a wire mesh.
9. **SDF Collision Events:** a bowl and sphere are inferred through stable
   sliding and bouncing. There is no systematic penetration or jitter; splash
   children are bounded and mist stays near impacts.
10. **Grid-accelerated Flocking:** local alignment and separation create two
    wing-like arcs, a maintained avoidance cavity, and split/rejoin behavior.
    Gold chasers affect nearby agents rather than the whole flock.

## Review cadence

Reviewers must not author the implementation. Review once after the technique
is delivered and again after performance tuning. Store the concept revision,
commit SHA, seed, budget, three lossless stills, motion capture, telemetry
summary, and both scorecards together in the benchmark evidence directory.
