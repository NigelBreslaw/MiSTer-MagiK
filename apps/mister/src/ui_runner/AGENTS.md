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

## Adding a full-screen navigation transition

The chart (`FullScreenTransitionStateChart`) answers who may render; the timeline
(`NavigationTransitionController`) answers how far the motion has got. They are
independent axes (roadmap PR 10): add to the timeline side, never fold one into the other.

1. Name it: a `NavigationTransitionRoute` variant and `label()`. A card/list edge also
   needs a `NavigationTransitionEdge` and a row in `navigation_transition_for_intent`
   (`transition_plan.rs`). A Settings-family page needs a `settings_page_depth` and a
   `settings_page_transition` row (`navigation_transition.rs`).
2. Geometry: a pure, tested function (`transition_plan.rs` or the scenes crate's
   `navigation` module). It depends on the committed navigation state only.
3. Start: one `TransitionStart` constructor in `transition_spec.rs` choosing the raster
   space, `StartPolicy` and assets. `NavigationTransitionRuntime::begin` is the only start.
4. Render: draw only, from the two snapshots and `TransitionAssets`; no allocation per frame.
5. Prove it: add the edge to `EDGES` in `transition_scenarios.rs`. Its source, endpoint,
   reverse and hygiene checks, and the director walks in `presentation_scenarios.rs`,
   then apply without new test code.

An effect that is not navigation (orientation, as the model) takes a chart owner and the
director's begin/capture/end operations instead; it never starts the chart by hand.
