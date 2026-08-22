# macOS UI Preview Plan

Status: implemented and extended with read-only mounted-card discovery,
Mac-local Catalog V3 rebuilds, production screenshot pack loading/downloading,
shared settings persistence, and HDMI/CRT layout profiles. The operator
workflow is documented in `apps/mister/UI_PREVIEW.md`.

## Objective

Create a macOS visual-development application for MiSTer MagiK that runs the
real compiled Slint launcher and the real Rust RGB565 composition code. It
should make UI design, interaction review, screenshots, and deterministic
visual tests fast without claiming to reproduce MiSTer performance or scanout
behaviour.

The first useful milestone must display the complete HDMI launcher composition,
including:

- the real Slint UI, fonts, layout, overlays, and animations;
- the Rust-painted Arcade list;
- RGB565 screenshot previews and their production transition;
- representative production screensavers;
- deterministic synthetic catalog and media fixtures.

The final RGB565 frame is presented in a native macOS window. Device-only
framebuffer ownership, FPGA routing, vblank latch, VT management, Linux input,
and Main handoff remain in the MiSTer runtime.

## Architectural Boundary

The preview must consume the same composition functions as the device runtime:

```text
Slint Launcher ──────────────┐
Rust Arcade renderer ────────┼─> UiFrameTarget RGB565 frame
Screenshot compositor ──────┤                │
Screensaver renderer ───────┘                ├─> MiSTer scanout adapter
                                             └─> macOS window adapter
```

The macOS application is not a second product UI and must not copy Slint
layouts, styling, or custom render algorithms. Its host-specific responsibilities
are limited to:

- constructing deterministic development scenarios;
- translating keyboard input into preview actions;
- scheduling redraws and fixed-clock frames;
- converting the final RGB565 frame for native window presentation;
- exporting deterministic captures.

The preview should be a dedicated macOS-only binary in the existing
`apps/mister` package. It should not be added to the desktop companion because
that application has a different UI, state model, and connection lifecycle.

## Commit Sequence

Each commit must build independently and preserve the production renderer's
existing behaviour.

### 1. `refactor(ui): separate portable RGB565 composition from device presentation`

Extract a narrow library-owned visual-composition boundary from the device
binary. Reuse the existing implementations rather than fork them.

Initial shared surface:

- Slint software rendering into `UiFrameTarget`;
- `ArcadeListRenderer` composition into the cached RGB565 frame;
- raw RGB565 screenshot composition;
- the fixed animation clock;
- screensaver and particle rendering needed by the later preview commit.

Likely source areas:

- `apps/mister/src/arcade_list_renderer.rs`
- `apps/mister/src/bitmap_text.rs`
- `apps/mister/src/preview_state.rs`
- `apps/mister/src/screenshot_transitions.rs`
- `apps/mister/src/ui_display.rs`
- `apps/mister/src/ui_runner/launcher_compositor.rs`
- `apps/mister/src/ui_runner/launcher_screensaver.rs`
- `apps/mister/src/ui_runner/particle_renderer.rs`
- `apps/mister/src/ui_runner/raw565_preview_renderer.rs`
- `apps/mister/src/ui_runner/ui_frame_target.rs`
- `apps/mister/src/ui_runner/ui_platform.rs`

Keep device presentation modules private to the device binary:

- framebuffer and scanout-slot mapping;
- FPGA route and latch publication;
- VT ownership;
- vblank pacing;
- launch and return handoff;
- Linux evdev input.

Acceptance:

- the production binary calls the extracted composition functions;
- existing composition tests remain at their current boundary or move with the
  extracted code;
- there is no intentional pixel or runtime behaviour change;
- bounded Rust analyzer diagnostics are clean for the coherent edit batch.

### 2. `feat(ui-preview): present the real launcher in a macOS RGB565 window`

Add a macOS-only `mister-magik-ui-preview` binary and a dedicated Cargo feature.
Add only the native-window dependency needed to present a CPU-owned pixel
buffer.

The application should:

- instantiate `mister_magik_ui::launcher::Launcher`;
- use the existing software-renderer platform and `UiFrameTarget`;
- render the actual 960x540 HDMI composition;
- convert the final RGB565 buffer for the native window surface;
- use nearest-neighbour enlargement and preserve aspect ratio;
- handle expose, resize, redraw, and clean window close;
- avoid initializing device layout, `/dev/fb0`, FPGA, VT, or Linux input.

The compiled UI is the primary path. A Slint interpreter must not replace it,
because that would bypass the generated bridge and parts of the real
composition pipeline.

Acceptance:

- the real Home screen opens in a native Mac window;
- resizing does not distort the framebuffer aspect ratio;
- a known RGB565 test pattern proves channel conversion and orientation;
- running the preview performs no device access.

### 3. `feat(ui-preview): add deterministic launcher scenarios`

Add a preview-owned scenario model backed by synthetic public data. Fixtures
must be deterministic, small, and free of private screenshots or device paths.

Cover:

- Home with enough systems to exercise scrolling;
- nested collection breadcrumbs;
- Controller and controller-setup states;
- Settings and display selection;
- Screensaver Settings;
- About, Info, and Licenses;
- startup, loading, compatibility, confirmation, catalog-scan, background-scan,
  and media-progress overlays;
- empty, loading, ready, and failed catalog states;
- long labels and missing optional metadata.

Populate the real typed launcher globals, `MenuItem`, `ArcadeGame`, and
`ScreenshotPackProgress` types. Do not create preview-only Slint properties.

Controls:

- number keys or a compact command palette select scenarios;
- arrow keys alter selection and scroll positions;
- a key toggles overlays;
- a key pauses or advances the fixed clock;
- the native window title shows route and scenario without modifying captured
  product pixels.

Acceptance:

- every Slint screen and overlay is reachable without MiSTer files;
- scenario selection is deterministic across runs;
- changing scenarios leaves no stale bridge state from the prior scenario.

### 4. `feat(ui-preview): compose real Arcade rows and screenshot previews`

Complete the first important mixed-composition vertical slice.

Use:

- a synthetic `ArcadeCatalog` containing enough games for scrolling;
- the real `LauncherNav` selection and visual scroll values where practical;
- the real `ArcadeListRenderer`;
- the real raw RGB565 preview renderer;
- the production screenshot fade transition;
- the same layer ordering and invalidation rules used by `MixedArcade`.

Provide procedurally generated RGB565 screenshot fixtures for committed tests.
Optionally accept an ignored local media root for visually inspecting real
screenshot packs; absence of that root must never prevent the preview starting.

Acceptance:

- the Mac window shows the Slint Arcade chrome, Rust-painted rows, selected-row
  treatment, and screenshot preview as one final frame;
- keyboard navigation updates both the list and preview;
- entering a full-screen overlay clears or covers direct layers exactly as the
  composition state requires;
- returning to Arcade repaints required direct layers in the same frame;
- the screenshot empty, loading, ready, clipped, and transitioning states are
  reviewable.

This commit completes the minimum useful daily UI-development application.

### 5. `feat(ui-preview): run production screensavers with fixture media`

Introduce an explicit image-source boundary for screensaver construction:

- the device implementation continues to load production screenshot archives;
- the Mac implementation supplies deterministic in-memory fixture images;
- particle and procedural modes use their existing engines unchanged;
- image-based modes use the same scaling, placement, and RGB565 rendering code.

The preview should expose mode selection, pause, single-frame advance, and a
fixed elapsed-time argument.

Acceptance:

- at least one particle mode, one procedural mode, and one screenshot-driven
  mode animate in the Mac window;
- fixed seed plus fixed elapsed time produces identical RGB565 frames;
- changing screensaver mode does not leave pixels from the prior mode;
- the preview performs no background scan of the developer's filesystem.

### 6. `test(ui-preview): add deterministic headless visual capture`

Make the preview runtime usable without opening a window:

```text
mister-magik-ui-preview \
  --scenario arcade \
  --frame 12 \
  --output /tmp/mister-magik-arcade.ppm
```

The headless path and interactive path must call the same scenario and
composition code.

Add coverage for:

- stable frame dimensions and RGB565 hashes;
- expected non-empty screen regions;
- Arcade list and preview layer ordering;
- screenshot scaling and clipping;
- overlay invalidation and repaint;
- fixed-clock transition frames;
- fixed-seed screensaver frames;
- HDMI and selected CRT composition geometries.

Prefer compact hashes, region assertions, and a small number of diagnostic
pixel assertions initially. Add reviewed golden images only where they produce
meaningfully better failure output; never stage private screenshots.

Acceptance:

- representative scenarios render deterministically in headless mode;
- a deliberate one-pixel renderer change fails the relevant assertion;
- capture output is written only to an explicit path or OS temporary directory
  and never overwrites an explicit existing file.

### 7. `dev(ui-preview): document and streamline the local workflow`

Add a human-facing development command under `apps/mister/scripts/` and document
it in the relevant developer documentation.

The workflow should provide:

- one command to open the default preview;
- route and scenario arguments;
- capture examples;
- keyboard controls;
- a process-level rebuild/restart loop for Rust and Slint changes;
- actionable handling when optional watcher tooling is unavailable;
- a clear list of device-only validation responsibilities.

Do not use Slint interpreter live reload as a substitute for rebuilding the
compiled launcher. Restarting the preview process is acceptable because it
keeps generated bindings and Rust composition honest.

Acceptance:

- a clean macOS checkout can launch the preview using the documented command;
- editing a `.slint` file rebuilds and reopens the compiled UI through the
  documented watch workflow;
- documentation explicitly states that Mac timing is not MiSTer performance
  evidence.

## Milestones

### Milestone A: useful visual development

Commits 1-4:

- real compiled Slint launcher;
- deterministic screen and overlay scenarios;
- real Rust Arcade text;
- real RGB565 screenshots and transition;
- interactive Mac window.

This is the first delivery target.

### Milestone B: broad visual coverage

Commit 5:

- representative real screensavers;
- deterministic time and media sources.

### Milestone C: repeatable regression testing

Commits 6-7:

- headless captures;
- stable visual assertions;
- daily rebuild/restart workflow.

## Assurance And Review

During Rust changes, use the repository-scoped Rust LSP for navigation and
coherent-batch diagnostics. Use the Slint MCP and actual rendered screenshots
for UI inspection.

For each commit:

1. run `scripts/agent plan` to inspect the assurance selected by its paths;
2. inspect the Git diff and stage only intentional paths;
3. commit through the repository's normal pre-commit gate;
4. push through the full affected pre-push assurance before treating the
   commit as complete.

The shared composition extraction affects production runtime code. After the
series is clean, run the normal development delivery and perform one focused
device visual smoke check covering Home, Arcade with a ready screenshot, a
full-screen overlay over Arcade, and a representative screensaver. This is a
regression check for shared code, not a requirement for ordinary Mac preview
iteration.

## Explicit Non-Goals

- Simulating Cortex-A9 performance or scheduler behaviour.
- Reproducing HDMI electrical output, FPGA scaling, tearing, or vblank timing.
- Emulating `/dev/fb0`, scanout slots, FPGA route ownership, or Main handoff.
- Replacing device controller-mapping tests.
- Loading the developer's real MiSTer filesystem by default.
- Creating a second set of Slint layouts or a preview-specific visual design.
- Treating macOS FPS or latency as MiSTer performance evidence.

## Principal Risks

### Accidental visual fork

Mitigation: the Mac binary consumes compiled production Slint and shared Rust
renderers. Preview-only code owns fixtures and presentation, not visual rules.

### Extraction destabilizes the device binary

Mitigation: move code without behaviour changes in the first commit, retain
existing tests, keep device adapters private, and perform a focused device
smoke check after the complete series.

### Screenshot and screensaver loaders retain device assumptions

Mitigation: inject narrow image sources at construction boundaries. Do not
introduce fake `/media/fat` trees or environment-variable path rewriting.

### Visual tests become brittle

Mitigation: use fixed seeds and clocks, assert purposeful regions and hashes,
and reserve full golden images for stable high-value scenarios.
