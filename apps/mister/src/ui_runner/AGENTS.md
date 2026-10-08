# Launcher runtime

Start with `launcher_loop.rs`, then the session owning the changed behavior.
Keep cache creation, network work, and blocking persistence off the UI path.
Isolate benchmark policy from production defaults.

A full Slint present invalidates direct Arcade layers: repaint them in the same
frame while Arcade remains active. Count a missing fresh animation pose and a validated protocol-v5
refresh repeat as `dropped_frames`, once per affected refresh. Exclude intentional
idle/settled reuse. Keep causal evidence and uncertainty; latch rejection is
a separate gate. Test actual event/state sequences, not helper predicates alone.

For unresolved ordering, read the matching `docs/architecture.md` section:
Boot And Process Model, Launcher Composition, Game Launch Handoff, or Launcher
Navigation Model. Physical scan-out, timing, input hardware, and Main handoff
claims require device evidence.

Animation time comes only from the launcher loop's `FrameClock` (one display
period per produced frame); never read `Instant::now()` for motion. See
`docs/architecture.md` Animation Time.
