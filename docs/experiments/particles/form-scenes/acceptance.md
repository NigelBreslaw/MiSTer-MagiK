# Archived Particle Form commercial acceptance

The Form scenes are complete only when two independent reviewers return
`SHIP` for every final FPGA-latched capture.

## Deterministic contract

- seed: `827141709451`
- framebuffer: 960x540 direct RGB565
- background: true black
- HUD: hidden
- duration: 30 seconds per scene
- stills: the entry, hero, and exit timestamps in `README.md`
- motion: continuous 5-8 second USB Video capture containing the hero beat
- authority: `fpga-latched-scanout-slots`; `/dev/fb0` is not evidence

Each reviewer scores composition, material, motion, depth, RGB565 handling,
and artifact freedom from 1-5. Acceptance requires `SHIP` from both reviewers
and every score at least 4.

The native 1x image must have readable black margins and no stale pixels,
flicker, popping, clipping, cohort seams, moire crawl, temporal shimmer,
palette collapse, depth inversions, or allocation-related cadence changes.
Performance tuning must not remove the scene's defining visual read.

## Physical performance gate

- unique physical presentation is within 0.1 FPS of 60 Hz;
- render P99 is below the measured refresh period minus 750 microseconds;
- no dropped frames, completion gaps, latch drops, presentation misses,
  starvation, unsafe slot reuse, or superseded frames;
- no allocation or capacity growth after three warm-up frames;
- the declared source, evaluated, visible, clipped, topology, attempted-write,
  and scratch high-water counts are present in evidence.

Qualify each scene individually for 30 seconds, then run
`particle-form-scenes`. Use `particle-form-scenes-profile` only for a failed
gate or a justified optimization. Review locally after every visual change and
again from FPGA-latched evidence after performance tuning.

## Scene-specific requirements

1. **Fractal Grid Terrain:** the regular lattice remains readable while its
   crest and overhang form. Depth and negative space must survive at native 1x.
2. **Layer-mapped Hologram:** the joystick silhouette, terraces, scan reveal,
   and slow rotation remain coherent without stippled edge collapse.
3. **Spherical Field Observatory:** the central void, orbiting flow, asymmetric
   depth, and sparse trails remain visible without unstable bursts.
4. **Twisted Multi-form Cathedral:** nested structures retain symmetry,
   architectural hierarchy, and a central void without segment tangles.
5. **Point-cloud Morph Passage:** spacecraft and manta endpoints hold exactly;
   the passage remains coherent and assignment arcs never form a wire mesh.

Store the commit SHA, concept revision, seed, budgets, stills, motion,
telemetry, and both scorecards together under ignored benchmark evidence.
