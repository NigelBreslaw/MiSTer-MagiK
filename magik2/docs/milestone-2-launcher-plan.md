# Mini launcher milestone 2: joystick browsing with identity-preserving slides

## Delivery and boundaries

Historical plan: the initial portable implementation used Luna. The user later
prohibited subagents; all further fixes, review and milestone 3 are performed
directly by the coordinating agent. No push is authorized.

Deliver a joystick-controlled, cyclic five-card carousel in Mini. Every move is
a slide: the side card becomes the centre card, with its label travelling with it.
No perspective, flips, artwork, catalog integration, game launching, settings UI,
or broad performance optimisation. Milestone 3 adds paired same-direction flips
on fresh presses, preferred reverse mapping, then slides while held.

Read applicable AGENTS and use the Rust LSP skill. Scope: shared scene renderer,
Mini consumer, consumer scenarios/docs, and a narrow runtime input adapter only
if needed to consume the existing Main-mapped input protocol. Do not import the
whole real app or alter tooling-core/native protocol/platform/Main. Report a
concrete blocker if those interfaces cannot support delivery.

## Preserve the latest baseline

- [ ] Inspect status and preserve all existing work. At planning time HEAD was
  `3db811152` and `launcher.rs` still contained our uncommitted visual cleanup.
  Verify its diff, then commit those existing changes separately as
  `style(mini-magik): simplify chrome and soften reflections` before new work.
  If already committed, do not duplicate the commit.
- [ ] Background is pure RGB565 black, including letterboxing.
- [ ] Do not restore Settings, Play / Collect / Remember, One Library / Every
  Generation, Across All Collections, or MiSTer / Offline text.
- [ ] Reflections are now 32 logical pixels, beginning at about 22% brightness,
  with a quadratic per-row fade and stationary 4x4 ordered dithering. Preserve
  dimness, black endpoint and no temporal noise; do not restore four fade bands.
- [ ] Preserve sidebar fixture totals, clock `21:37`, category order and colours,
  selected cream double outline, header/footer and remaining copy.
- [ ] Preserve Main-authoritative display planning: 960x540 source at current
  1080p mode, `open_for_plan` destination 1920x1080. No CPU upscale to 1080p,
  source-sized destination, raw FPGA timing inference, or display mode change.
- [ ] Inspect the latest native baseline capture before editing:
  `build/magik2-results/20260907T194744Z-cc0eaa4fb3cd/smoke.png`.

## Commit 1: `feat(framebuffer-scenes): add launcher slide state and composition`

### Navigation contract

- [ ] Add a portable deterministic navigation state machine driven by explicit
  timestamps and logical left/right press/release state; no Slint/device types.
- [ ] Right selects the next category and moves the strip left; Left does the
  opposite. Wrap Arcade, SNK NeoGeo, Consoles, Handhelds, Computers cyclically.
- [ ] Fresh press starts immediately. Initial defaults: 180ms tap slide, 300ms
  hold threshold from press, 150ms per subsequent held step. Named constants,
  not scattered magic numbers; record any device-driven tuning.
- [ ] Release finishes only the in-flight step; no extra release animation or
  repeat backlog. Hold repeats start only at step boundaries after threshold.
- [ ] Direction reversal finishes the in-flight step, then honours the latest
  opposite intent. At most one pending discrete press; repeated OS key-repeat
  events must not enqueue moves. Multiple new taps replace that bounded intent.
- [ ] Both directions held cancel further movement; finish the current step.
  Disconnect, mode exit and input reset clear held/pending intent. Require neutral
  after startup/reconnect/mode entry so inherited held input cannot run away.
- [ ] Distinguish settled selection from the in-flight target. Advance settled
  selection once per completed slide; no clock-jump catch-up across unseen cards.
- [ ] Tests: taps, short release, multi-second hold, five-card wrap both ways,
  reversal, simultaneous directions, bounded rapid taps, reconnect/reset, empty
  and single-card data, irregular/large time advances and settled idle.

### Rendering contract

- [ ] Prepare/cache static chrome and reusable card content. Render moving frames
  into reusable buffers; no per-frame texture rebuild, text formatting or heap
  allocation. Keep fixtures in Mini, not in the portable renderer.
- [ ] Replace hard-coded slot switching with interpolated card rectangles and
  continuous content transforms. Retain the current rest layout where possible.
  Incoming title moves/scales from its side placement to centre placement;
  outgoing title does the reverse. Never swap labels at the halfway point.
- [ ] Fade game count/ordinal/outline as needed, but category identity remains
  visible throughout. Do not substitute a whole face abruptly during a slide.
- [ ] Use integer-snapped raster positions and nearest-neighbour sampling. Avoid
  bounce/overshoot. Smooth tap easing; held steps must not add a dwell at every
  centre. Endpoint geometry must exactly match the next resting composition.
- [ ] Include offscreen neighbours for seamless wrap, clipping strictly to the
  carousel. No edge teleport visible inside the clip; deterministic overlap and
  draw order, especially while two cards trade centre prominence.
- [ ] Reflect the current transformed card bottoms, with matching movement,
  clipping and depth order. Read card pixels only, never reflection pixels.
  Keep dithering deterministic in screen coordinates and black outside the fade.
- [ ] Sidebar, header and footer stay fixed; only selection indicator changes
  with settled selection. Clear/restore the whole dirty carousel area so old
  cards/reflections cannot leave trails. Full scanout copies remain acceptable;
  do not claim partial hardware updates just because chrome is cached.
- [ ] Retain proportional logical fitting tests for 960x540 and 640x480.
- [ ] Add start/mid/end and wrap rendering tests for continuity, clipping,
  unchanged chrome, reflections and deterministic output; preserve existing tests.

## Commit 2: `feat(mini-magik): drive launcher slides from mapped input`

- [ ] First resolve the existing mapped physical input contract. Main already
  supplies a virtual input device and the native service sets
  `MISTER_MAGIK_INPUT_PROXY` / `MISTER_MAGIK_INPUT_PROXY_PROTOCOL` when available.
  Inspect `apps/mister/src/input_hub.rs`, portable input event/reducer code, and
  the service handoff read-only; reuse protocol semantics, not the entire app.
- [ ] Do not silently call `FramebufferLabInput::open()` and label it mapped:
  that adapter uses `InputProfile::guess` on raw joystick nodes. Do not hardcode
  a controller layout or consume both raw and mapped streams (duplicate moves).
- [ ] If needed, put a small nonblocking Main input adapter in runtime, reuse
  shared wire/reducer types, and add focused parsing/reset tests. Honour packet
  boundaries, release/reset/disconnect and supported protocol versions. Missing
  capability is an explicit diagnostic, never fallback to guessed controls.
  Stop/report if this requires tooling-core or Main changes.
- [ ] Poll input and service tooling while moving AND idle. Schedule rendering
  only for changed animation state; no continuous identical posts when settled.
- [ ] Feed physical input and invisible test actions through the same navigation
  state machine. Expose left-down/up, right-down/up and reset actions for testing;
  no visible buttons, touch dependencies or synthetic navigation shortcuts.
- [ ] Preserve show-probe/show-launcher and existing demo tests. Leaving launcher
  cancels its animation/input state; entering it clears stale held state. Probe
  callbacks must not mutate the launcher or leave its demo timer running.
- [ ] Expose test-readable selected category/index, target, idle/sliding/held
  state, input readiness/source, and fixture status. Settled-ready must reflect
  a successfully latched final frame, not just a CPU state update.
- [ ] Keep HiddenLatchPresenter, safe RGB565 boundary conversions, stride-aware
  copies, Main display geometry and preview of the same composed pixels.
- [ ] Use existing latch/metrics semantics. Pace against actual presentation;
  separate rejected posts from physical drops/repeats. On failed final post keep
  the scene dirty/recoverable and report error rather than falsely declaring idle.
- [ ] Keep rendering and preview preparation separate; no preview reconstruction
  added to the hot path. No premature NEON/threading/presenter redesign.
- [ ] Run focused Mini and renderer checks through scripts/cargo and bounded
  LSP diagnostics. Dependency edits only via dependency-sync workflow, staging
  owning manifest and adjacent lockfile only.

## Commit 3: `test(mini-magik): validate joystick carousel on device`

- [ ] Add bounded consumer smoke coverage for initial Arcade, right/left taps,
  cyclic wrap and return to idle; use accessibility for state only. Keep smoke
  short. Capture the native latched frame after a settled noninitial selection.
- [ ] Add a separate explicitly invoked launcher motion journey: several seconds
  held each way, release, reversal and repeated taps using the test input actions.
  Preserve the existing probe workload as separately named evidence; never claim
  its timings as launcher performance. Do not modify tooling-core to add selectors.
- [ ] Test idle after browsing: same artifact/PID, fresh elapsed interval, no
  additional posts/presentations, no error, correct source/destination geometry.
- [ ] Run focused affected Python and Rust checks only, not workspace matrices.
- [ ] Deploy `scripts/magik2 deploy --app mini-magik`, then
  `scripts/magik2 check --app mini-magik`. Run the bounded launcher motion
  scenario through existing typed 2.0 selection after inspecting its CLI support.
  Use first-attempt escalation for all device/container operations.
- [ ] Verify real joystick left/right input separately from automation. If no
  supported typed physical-input route is available, ask the user for a brief
  controller test; do not equate accessibility injection with physical proof.
- [ ] Record launcher-specific frame/latch evidence for a bounded hold window;
  zero dropped frames required for authoritative smooth-animation acceptance.
  No captures/tree polling inside the timing window. If evidence is insufficient
  or nonzero, report it and fix only narrow milestone issues; do not relabel failure
  as success or begin a broad optimisation project.
- [ ] Inspect actual native captures for text continuity where capture supports
  it, selected placement, reflection fade/trails, clipping and wrap. Static
  captures alone are not proof of smooth motion. Retain evidence untracked.
- [ ] Document commands/results, actual framebuffer geometry, input authority,
  timings, physical-test status, limitations and deferred milestone 3. Update
  this checklist to distinguish completed items from any blocked acceptance.

## Handoff and review

- [ ] Stage exact paths and escalate staging/committing on the first attempt.
  Commit in the order above; no push and no unrelated changes/history rewriting.
- [ ] Send the coordinator commit IDs, changed paths, focused check results,
  device result directories, native capture path, measured drops/rejections and
  any remaining manual acceptance. Do not claim completion without evidence.
- [ ] Coordinator independently reviews state transitions, mapped input handling,
  renderer hot path, wrap/draw order, final-latch readiness and smoke assertions;
  returns concrete fixes to Luna and checks the corrected result.
- [ ] Final response embeds the device capture and explicitly reports physical
  joystick/motion acceptance or its remaining blocker. Stop before flips.
