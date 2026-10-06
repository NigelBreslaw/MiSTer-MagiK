# Presentation And Transition Refactor Roadmap

Status: proposal (2026-10-06). Goal: full-screen transitions that are
(a) reliable by construction and (b) cheap to add, with one state chart that
owns what is on screen, who produces the next frame, and how much of it must
be repainted.

## Why the same glitches keep returning

Two current bugs, both traced to code:

1. **Home → Arcade: cards jump at the start of the reveal (HDMI landscape).**
   The device-card reveal snapshots `target.cached_565()` as its source
   (`launcher_loop.rs`, the `begin_device_card(...)` call in the
   `OpenCollection` branch). Card-home motion presents directly into scanout
   slots, so that cache can still hold an older carousel pose. The transition
   therefore starts from pixels the user never saw. The geometry is patched
   with `cards.selected_card_rect()`, but the pixels are not. The Settings cog
   path hit exactly this in #190 and fixed it locally by rendering
   `launcher_card_home.render()` as the source ("the generic cache may still
   contain a neighbour"). The fix never became a shared rule.

2. **Arcade → game → return → Back: no card animation, just a fade.**
   Reverse transitions get their geometry from
   `NavigationTransitionRuntime::geometry_for_reverse`, an in-memory stack
   pushed by forward transitions. Returning from an FPGA game is a new launcher
   process (`StartupMode::ReturnFromGame`). `LaunchReturnState` restores
   navigation but not that stack, so the reverse lookup returns `None`, no
   transition starts, and the intent is committed as an instant cut. The only
   motion left is ordinary preview/layer settling, which reads as a crossfade.
   (Root cause comes from reading the code. The exact source of the visible
   fade still needs confirmation on the device.)

Both bugs are the same class: **each transition chooses its own source pixels,
geometry, and lifetime rules**. Nothing enforces continuity with the frame
that is actually on screen.

## What exists today

| Concern | Owner today | Problem |
| --- | --- | --- |
| Frame-loop coordination | `run_launcher_loop`, about 8,900 lines (`launcher_loop.rs` 5193–14051), about 150 top-level `let mut` locals | Transition lifetime is spread across locals such as `pending_navigation_transition`, `navigation_transition_generation`, `orientation_transition_generation`, and `full_screen_transition_live_endpoint_rendered` |
| Render policy (timers, capture, lock, release) | `FullScreenTransitionStateChart` | Good kernel, but only Navigation and Orientation use it. The `StartupReveal` and `Screensaver` owners are declared and never used. It controls policy, not lifecycle |
| Navigation playback | `NavigationTransitionRuntime` + `NavigationTransitionController` (phases Idle/Capture/Expand/Covered/Reveal/Reversing/Settled) | Seven `begin_*` entry points (`begin`, `begin_physical`, `begin_device_card`, `begin_system_panel`, `begin_arcade_card`, `begin_settings_page[_physical]`, `begin_settings_cog_physical`), each with its own source, space, start-immediately vs deferred, and history rules. `begin_arcade_card` / `ArcadeCard` was reached only from tests (now deleted) |
| Card level trick | `launcher_card_home` + `CardLevelHandoff` | A separate transition system that presents straight into scanout and bypasses the shared chart |
| Orientation | `orientation_transition.rs` | Separate crossfade/zoom implementation |
| Screensaver | `blend_screensaver_crossfade` in the compositor | Separate crossfade |
| Startup reveal / particle morph | `startup_intro.rs` + lifecycle startup chart | Separate handoff rules |
| Composition / direct layers | `UiCompositionController` (8 states) + retirement region | Sound, but independent of the transition chart. The loop keeps them in sync by hand |
| Damage / partial render | Decided inline per path (`full_frame_present`, `NavigationDestination` full raster, card `chrome_copy_damage`, CRT overlay deltas) | No single declaration of what each state must repaint |

The codebase already has the right instincts, with typed enum charts, generations,
and physical-present receipts. What's missing is **one owner above them**, and
a **transition protocol** that every full-screen effect implements.

## Target architecture

```mermaid
flowchart LR
    Input[Input / intents] --> Director
    Workers[Workers: catalog, prep, artwork] --> Director
    Present[Present receipts] --> Director
    Director[PresentationDirector<br/>hierarchical state chart] -->|FramePlan| Producer
    Producer[Frame producers<br/>Slint, card home, transition renderer, screensaver] --> Presenter
    Presenter[Presenter / latch] -->|PresentedFrame + receipt| Present
    Presenter -->|PresentedFrame| Director
```

### 1. One presentation chart with parallel regions

`PresentationDirector` owns a single hierarchical chart. It is hand-rolled typed
enums, allocation-free, and pure: `handle(event) -> Effects`, like
`lifecycle.rs`.

```mermaid
stateDiagram-v2
    state Launcher {
    state Presentation {
        [*] --> Live
        Live --> Transition: Begin(spec)
        Transition --> Live: released + physically presented
        Live --> Screensaver
        Screensaver --> Transition: Begin(screensaver exit spec)
        Live --> Modal
        Modal --> Live
        Live --> Handoff: launch game
        state Transition {
            [*] --> AcquireSource
            AcquireSource --> Preparing: source = last presented frame
            Preparing --> Playing: destination ready / start-immediately
            Playing --> Covered
            Covered --> Revealing: destination captured
            Playing --> Reversing: reverse / cancel / timeout
            Revealing --> Reversing
            Reversing --> Settled
            Revealing --> Settled
            Settled --> Releasing
        }
    }
    --
    state Route {
        Home --> Arcade
        Arcade --> Home
        Home --> Settings
    }
    --
    state Composition {
        FullSlint --> MixedArcade
        MixedArcade --> FullSlint
    }
    --
    state DirectLayerRetirement {
        Idle --> Pending
        Pending --> Idle
    }
    }
```

The existing `FullScreenTransitionStateChart`, `UiCompositionController`,
`NavigationTransitionController` phases, and direct-layer retirement become
**regions or substates of this one chart** rather than separate objects the
loop reconciles. Owners, generations, and physical-present confirmation stay
exactly as they are today.

### 2. A transition protocol, not per-transition plumbing

```rust
struct TransitionSpec {
    kind: TransitionKind,          // CardReveal, SystemPanel, SettingsSlide, SettingsCog,
                                   // CardLevelTrick, Orientation, Screensaver, StartupMorph
    direction: Direction,
    space: RasterSpace,            // Logical | Physical(axis) — decided once
    start: StartPolicy,            // Immediate | AfterDestinationReady { timeout }
    geometry: TransitionGeometry,  // derived, never remembered (see 3)
    duration: FrameCount,          // FrameClock periods
}

trait TransitionRenderer {
    fn prepare(&mut self, spec: &TransitionSpec, assets: &Assets);
    fn render(&mut self, t: Progress, src: &Frame, dst: Option<&Frame>, out: &mut Frame) -> Damage;
}
```

The shared machinery owns everything the renderers keep re-implementing: source
acquisition, destination capture, start/deferral, reversal, cancel, timeout,
CPU quiescence, Slint timer freeze, release confirmation, benchmarks, and
telemetry. One `begin(spec)` replaces the seven `begin_*` variants. A renderer
only draws.

### 3. Three invariants that kill the recurring bugs

- **Source continuity:** the source of every transition is the
  `PresentedFrame` the presenter last confirmed, whichever producer drew it
  (Slint cache, card-home slot, direct layers composed). No call site passes
  `cached_565()` directly. *Fixes bug 1 and the whole "jump at start" class.*
- **Derived geometry:** forward and reverse geometry come from one pure function
  `transition_geometry(edge, route_snapshot, layout, card_rects)`. Reverse never
  depends on history in process memory, so it survives game return, orientation
  changes, and a cleared stack. *Fixes bug 2 and the whole "fell back to a cut"
  class.* If something truly can't be derived, it goes in `LaunchReturnState`.
- **Endpoint continuity:** the last transition frame equals the first live frame
  of the destination. This is already enforced for navigation via
  `NavigationDestination`. It becomes a chart rule for every kind.

### 4. Damage policy is declared by state, not decided per path

Every chart state declares its `DamagePolicy`, and producers obey it:

| State | Policy |
| --- | --- |
| Live / FullSlint | Slint damage (partial) |
| Live / MixedArcade | Slint damage + direct-layer regions; full Slint present repaints layers in the same frame |
| Live / card browsing | Card band + chrome damage |
| Transition / AcquireSource, Playing, Reversing | Renderer-reported damage over a full-frame transition buffer; Slint frozen |
| Transition / Covered → Revealing (destination capture) | One forced full raster of the destination (today's `NavigationDestination`) |
| Transition / Releasing | One forced full live raster, then back to partial |
| Screensaver | Full-frame owner, direct layers cleared |

A host test asserts **damage soundness**: for any event sequence, the
incremental result equals a forced full raster of the same state. That turns
"doesn't understand partial rendering" into a failing test, not a device
surprise.

## Progress

PR numbers below follow the phases: PR 0 is Phase 0, PR 1 is Phase 1, and so on.

**PR 0 (merged, #234): fix both bugs and start the shared planning module.**

- `launcher_runtime/transition_plan.rs` holds the shared rules: `card_home_owns_source` picks the
  producer of the visible frame, and `derive_navigation_geometry` gives geometry for both
  directions from committed state.
- `LauncherNav::menu_tile_view` is the tile a transition collapses into. Inside a collection
  `nav.selected` is a catalog-system index ("transitional compatibility"), not the menu tile, so
  geometry must use the level's remembered view, which is also what Back restores.
- The remembered geometry stack (`geometry_history`, `geometry_for_reverse`,
  `clear_geometry_history`) is deleted, as is the unused Arcade-card navigation renderer. The
  cabinet artwork and `ArcadeCardRenderer` stay: device cards, the settings cog and
  `visual-concepts` use them.
- `launcher_runtime/transition_scenarios.rs` drives every card-capable edge in both directions
  from a cold runtime: source continuity, endpoint continuity, mid-flight reversal, and reuse.

**PR 1 (merged, #235): scenario harness at navigation level.**

- The route table (`navigation_transition_for_intent`) and the geometry assembly
  (`navigation_geometry`, `crt_navigation_layout`, `NavigationDisplay`, `TileView`) live in
  `transition_plan.rs`. The device loop and the macOS preview both call them.
- Navigation scenarios run the real `LauncherNav` and assert that every reverse reveal replays its
  forward geometry, with and without the carousel's card rectangle.
- A review found that portrait transitions could begin from the logical card-home frame; fixed.

**PR 2 (merged, #236): the transition start path runs from a test.**

- `ui_runner/launcher_transition_start.rs` holds `begin_navigation_transition`, extracted from
  `run_launcher_loop` with no behaviour change. It takes plain inputs (navigation state, layout, the
  card-home session, the composed cache) and chooses the source pixels, the geometry and the renderer.
- Host tests drive it with a real card-home session: Home to Arcade begins from the card-home frame
  and not the stale cache; portrait and reverse-from-Arcade begin from the composed cache. Making
  either choice wrong fails a test.
- A seeded random walk over the real `LauncherNav` (200 walks, 40 steps, with simulated process
  restarts between a collection and Back) checks every reverse reveal against its forward geometry.
- Still to do in Phase 1: screensaver, modal and orientation scenarios; damage soundness
  (incremental result equals a forced full raster). The preview still has its own start path, since
  it wraps a different card-session type; folding it in is a Phase 2 task.

**PR 3 (in progress): dither parity for CRT and portrait cards (Phase 3b step 1).**

- Responsive faces now dither their rotated poses at projection, as HDMI landscape does. The
  face-on resting pose stays exact, because those faces are already dithered once when baked and
  the resting labels must reach the output without resampling.
- `Face.dithered: bool` becomes `Face.dither: Dither { Off, MovingPoses, Always }`.
- Evidence: a resting selected card is pixel-identical before and after; receding and moving cards
  now show the same fine dither texture as HDMI landscape instead of flat, banded surfaces. All 21
  HDMI landscape review frames are byte-identical. The approved raster hashes for the portrait and
  CRT scenes are refreshed in all four feature combinations.
- Not measured: projection cost on the device. The dithered kernels already run at HDMI landscape's
  raster size; the CRT and portrait rasters are no larger, but a CRT or portrait device run of
  `scripts/magik check motion` is still owed.
- `examples/launcher_layout_review.rs` no longer compiled on `main`; it is fixed so the review
  renders can be reproduced.

## Phased plan

Each phase ships on its own and is checked with `scripts/magik check` on
device. There is no big-bang rewrite.

### Phase 0: fix both bugs the way the target architecture would (done, #234)

See Progress.

### Phase 1: scenario harness and continuity invariants

- A host-side scenario runner drives the real navigation state and transition
  runtime (the loop's per-frame state is not drivable from a test yet; making it
  so is part of this phase) through scripted event sequences: every
  route in both directions; reverse mid-flight; cancel; timeout; screensaver
  during and after; modal; orientation; return-from-game seeding; cold vs warm
  destination.
- Assert on every run: source continuity, endpoint continuity, no
  `Recovering`, damage soundness, and release confirmed.
- Add a property-based sweep over random event sequences. This is the
  safety net for every later phase.

### Phase 2: one transition protocol for navigation

- Introduce `TransitionSpec` / `TransitionRenderer`. Port super-scaler, device
  card, system panel, settings slide/segmented, and settings cog behind one
  `begin(spec)`.
- Fold `NavigationTransitionController` phases into the shared transition
  substate. `FullScreenTransitionStateChart` becomes the lifecycle owner, not
  just a policy table.
- `navigation_transition_for_intent` + `settings_page_transition` become a single
  route-to-spec table.

### Phase 3: move the other effects onto the protocol

- Orientation (fade/zoom), screensaver enter/exit crossfade, startup particle
  morph (activating the unused `StartupReveal` / `Screensaver` owners), and
  the card level trick.
- The card level trick is the hardest because it presents straight into
  scanout. It should still go through the chart as an owner with a
  `CardBand` damage policy, so other transitions can see what's on screen.

### Phase 3b: one card pipeline for HDMI and CRT

Principle: the bridge already feeds both display types the same data. Differences between HDMI
and CRT should be explicit presentation parameters (safe insets, pixel aspect, fonts, geometry),
not separate implementations. Every card improvement must reach both for free.

Today there are two card pipelines, selected by `LauncherScene::uses_responsive_layout`
(`launcher.rs`), which is true for CRT and for portrait (including HDMI portrait):

| Stage | HDMI landscape (960x540) | CRT and portrait (`launcher/responsive.rs`) |
| --- | --- | --- |
| Artwork | `.cardtex` or 360x504 art, kept high precision | `native_surface`: filtered to the native card size, reduced to RGB565 once |
| Face dithering | `Dither::Always`: ordered dither at projection | `Dither::MovingPoses` (since PR 3): dithered when rotated, exact when face-on |
| Layout and text | fixed canvas, role fonts | native raster, `Fonts::Uniform` / `Roles`, bitmap labels baked at output size |

The newer performance work (cardtex, fast quantisation, dithered projection kernels) lives on the
HDMI landscape side, so CRT and portrait do not get it.

Plan: keep `responsive::Layout` as the one geometry and text description (it already covers
CRT, portrait and 5:4), and make it produce the same face type the HDMI path does: high-precision
texels, `dithered = true`, `.cardtex` support at native sizes. Then HDMI landscape becomes one more
`Layout` rather than a special case, and `responsive.is_none()` branches in `prepare` disappear.

Order: (1) dither parity for responsive faces, behind the existing benchmark and the layout review
renders; (2) native-size `.cardtex`; (3) route HDMI landscape through `Layout`; (4) delete the
fixed-canvas path. Each step is gated by `scripts/magik check motion` and card fixtures, since the
projection kernels are on the 60 fps budget.

### Phase 4: PresentationDirector and loop extraction

- Merge `UiCompositionController`, the transition chart, and direct-layer
  retirement into one `PresentationDirector` with parallel regions and
  per-state `DamagePolicy`.
- Move the transition, composition, and present-receipt locals out of
  `run_launcher_loop` into the director. The loop shrinks to: collect events →
  `director.handle` → run effects → produce `FramePlan` → present →
  `director.on_presented(receipt)`.
- Update `docs/architecture.md`: the Mermaid charts become one chart. Add a
  test that every `(state, event)` pair in the code is listed in the doc table.

### Phase 5: a recipe for the next transition

Adding a full-screen transition should mean:

1. A `TransitionKind` variant and a row in the route-to-spec table.
2. A geometry function (pure, tested).
3. A `TransitionRenderer` (draw only).
4. One row in the scenario matrix. The continuity, damage, reverse, cancel, and
   release tests then apply automatically.

Document this in `apps/mister/src/ui_runner/AGENTS.md`.

## Risks and guardrails

- **Frame budget:** the protocol must stay allocation-free with no extra copies.
  `PresentedFrame` should reference the existing slot or cache, not copy it.
  Check every phase with the existing dropped-frame and motion benchmarks.
- **CRT vs HDMI vs portrait:** `RasterSpace` must be decided once in the spec.
  Today it is split across `begin` and `begin_physical`, which is the next likely
  source of one-off bugs.
- **Physical truth:** the host harness proves logic and pixels. Cadence and
  scanout still need device evidence, as the repo already requires.
