# Archived particle experiments

This area preserves particle research without including it in the production
MiSTer MagiK application.

The archived set contains:

- the 36-demo interactive showcase;
- Fireworks V1 and V2 plus their authored JSON recipes;
- particle-material and Form-style experiments;
- the standalone particle-technique preview binary;
- the original acceptance notes and concept-image metadata.

The code is available only through the explicit `experiments` feature. The
normal `ui` and `ui-preview` builds do not expose the showcase, compile its Rust
modules, embed its firework recipes, or enable its showcase-only ARM kernels.

Two particle sequences remain production-owned for a future startup animation:

- `apps/mister/src/particle_engine.rs` and `particle_renderer.rs` implement the
  CRT-noise-to-3D **MagiK** formation;
- `apps/mister/src/startup_particles.rs` contains the extracted arcade-cabinet
  cloud decoder, camera and point renderer.

The licensed arcade source model and compiled particle cloud remain under
`apps/mister/assets/particles/` because they belong to that retained startup
sequence. No pre-rendered frames or videos are used.

Large concept PNGs are deliberately not version-controlled. Existing local
copies were moved to the ignored `build/particle-experiments/` directory.
