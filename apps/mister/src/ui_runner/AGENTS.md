# Launcher runtime

Start with `launcher_loop.rs`, then the session owning the changed behavior.
Keep cache creation, network work, and blocking persistence off the UI path.
Isolate benchmark policy from production defaults.

A full Slint present invalidates direct Arcade layers: repaint them in the same
frame while Arcade remains active. Only a validated protocol-v5
`repeated_vblank_count` delta may populate `dropped_frames`; latch rejection is
a separate gate. Test actual event/state sequences, not helper predicates alone.

For unresolved ordering, read the matching `docs/architecture.md` section:
Boot And Process Model, Launcher Composition, Game Launch Handoff, or Launcher
Navigation Model. Physical scan-out, timing, input hardware, and Main handoff
claims require device evidence.
