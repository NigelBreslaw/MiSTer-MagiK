# AGENTS.md - launcher runtime

This directory owns launcher scheduling, lifecycle, workers, composition,
Slint bridging, and benchmark scenarios. Start with `launcher_loop.rs`, then
the named session or lifecycle module for the behavior being changed.

- Preserve lifecycle, composition, and display-ownership invariants.
- Keep cache creation, network work, and blocking persistence off the UI path.
- Keep benchmark-only policy isolated from production defaults.
- A full Slint present invalidates direct Arcade layers; repaint them in the
  same frame while Arcade remains active.
- Latch rejection and physical dropped frames are separate gates. Only a
  validated protocol-v5 `repeated_vblank_count` delta may populate
  `dropped_frames`; authoritative animation requires zero FPGA repeats.
- Test event/state sequences rather than helper predicates alone.

If source and sequence tests cannot resolve ordering, consult only the matching
`docs/architecture.md` heading: Boot And Process Model, Launcher Composition,
Game Launch Handoff, or Launcher Navigation Model. Device evidence is needed
only for scan-out, timing, input hardware, or Main-handoff claims.
