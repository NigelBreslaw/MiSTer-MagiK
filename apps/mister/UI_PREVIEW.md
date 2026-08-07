# macOS UI Preview

The macOS preview runs the compiled production Slint launcher and the shared
Rust RGB565 composition code in a native Mac window. It is intended for fast
UI design, visual review, and deterministic capture—not MiSTer performance or
scanout qualification.

## Start the preview

From the repository root:

```bash
apps/mister/scripts/dev-ui-mac.sh
```

Start on a particular scenario:

```bash
apps/mister/scripts/dev-ui-mac.sh --scenario arcade
apps/mister/scripts/dev-ui-mac.sh --scenario screenshot-screensaver
apps/mister/scripts/dev-ui-mac.sh --scenario particle
```

Run a navigation transition directly, with an optional slowed debug duration:

```bash
apps/mister/scripts/dev-ui-mac.sh \
  --navigation-transition-demo home-arcade \
  --navigation-transition-duration-ms 4000
```

Super-Scaler Shell is the only live navigation transition. The duration option
accepts 100–10000 milliseconds and also applies to deterministic frame captures.

To smoke-test a mounted MiSTer card, let the preview discover the single valid
card under `/Volumes`, or select it explicitly:

```bash
apps/mister/scripts/dev-ui-mac.sh --content card --sd-root /Volumes/MiSTer_Data
```

Card mode loads the newest valid Catalog V3 generation from the per-card Mac
cache, production card directory, or development card directory. Interactive
sessions then rebuild the catalog in the background with the production
scanner. Physical card paths are remapped back to `/media/fat` before
publication, so launch references are identical to MiSTer.

The card is always a read-only content source. Catalog state, scanner caches,
downloaded screenshot packs, settings, and temporary work are written beneath
`~/Library/Caches/MiSTer MagiK/ui-preview` and
`~/Library/Application Support/MiSTer MagiK/ui-preview`. Nothing in card mode
writes beneath `/Volumes`.

Use the offline controls when isolating a visual check:

```bash
apps/mister/scripts/dev-ui-mac.sh \
  --content card \
  --sd-root /Volumes/MiSTer_Data \
  --no-scan \
  --no-download \
  --scenario arcade
```

If a selected screenshot is absent from both the Mac cache and card, an
interactive session uses the production manifest downloader, checksum/index
verification, atomic publisher, media-state writer, and Catalog V3 availability
reconciler. Its destination roots are the Mac cache. Headless captures never
start downloads.

The screenshot screensaver follows the same media precedence. Card mode resolves
the Arcade 320x320 pack from the Mac cache, production card, or development
card, then constructs the production `LauncherScreensaver`. Its archive-backed
parade, depth layers, starfield, motion schedule, Lanczos scaling worker, card
rounding, and HDMI/CRT sampling profiles are the same Rust implementation used
by the MiSTer launcher. Removing the card cancels an in-flight open and retains
the last complete renderer.

Interactive previews use the current monitor refresh rate, capped at 120 Hz.
Force a target when comparing motion:

```bash
apps/mister/scripts/dev-ui-mac.sh --refresh-rate 60
apps/mister/scripts/dev-ui-mac.sh --refresh-rate 120
```

With `--refresh-rate auto` (the default), moving the window to another monitor
updates the target shown in the window title. An unfocused preview releases held
input and pauses its clock.

The default display profile is the production HDMI layout. The CRT profile
selects the production CRT Slint variant and CRT navigation metrics:

```bash
apps/mister/scripts/dev-ui-mac.sh --display-profile hdmi
apps/mister/scripts/dev-ui-mac.sh --display-profile crt --scenario arcade
```

The CRT profile is for layout and clipping review. macOS does not emulate
MiSTer video modes, FPGA scaling, direct-video timing, vblank/latch behaviour,
or the physical CRT raster.

If `cargo-watch` is installed, rebuild and restart the compiled preview when
Rust or Slint files change:

```bash
apps/mister/scripts/dev-ui-mac.sh --watch --scenario arcade
```

Without `cargo-watch`, rerun the ordinary command after saving. The preview
always rebuilds compiled Slint bindings; it does not substitute an interpreted
copy of the UI.

## Keyboard controls

| Key | Scenario |
| --- | --- |
| `1` | Home |
| `2` | Settings |
| `3` | Controller |
| `4` | About |
| `5` | Licenses |
| `6` | Info |
| `7` | Screensaver settings |
| `8` | Startup overlay |
| `9` | Confirmation overlay |
| `0` | Catalog scan |
| `A` | Arcade with Rust-painted rows and RGB565 preview |
| `B` | Background catalog scan |
| `C` | Compatibility banner |
| `L` | Launch/loading overlay |
| `M` | Media progress |
| `S` | Controller setup |
| `P` | Production particle screensaver |
| `T` | Production archive-backed screenshot screensaver |

Arrow-key presses and releases drive the production launcher navigation,
including velocity, turbo re-press, and spring settlement. `Space` pauses a
screensaver; `.` advances one refresh interval while paused. On Home, `Up`
focuses the Settings gear and `Enter` opens it; `Down` returns to the system
tiles. `Enter` opens systems and supported subpages, including Settings →
Screensaver → Preview. The screenshot screensaver returns to the launcher view
on input. `Escape` or `Backspace` goes back. The number shortcuts also work on
the numeric keypad.

## Deterministic captures

The capture path must not already exist:

```bash
apps/mister/scripts/dev-ui-mac.sh \
  --scenario arcade \
  --orientation monitor-clockwise \
  --refresh-rate 120 \
  --frame 12 \
  --output /tmp/mister-magik-arcade.ppm
```

Use `--orientation normal`, `--orientation monitor-clockwise`, or
`--orientation monitor-counterclockwise` to exercise the launcher geometry for
the corresponding physical monitor mounting. Portrait captures retain the
landscape composition dimensions sent to the display; rotate the captured image
in the mounting direction when reviewing it on an unrotated monitor.

Useful capture scenarios include `home`, `arcade`, `settings`,
`orientation-choice`, `arcade-crossfade`, `controller-setup`, `catalog-scan`,
`particle`, and `screenshot-screensaver`. Captures use a fixed animation clock
and deterministic in-memory catalog/media fixtures by default. Pass `--content
card --sd-root PATH --no-scan --no-download` for a bounded capture of real card
data. Headless `auto` uses 60 Hz; at an explicit 120 Hz, frame 12 is exactly 100
ms. Repeating a scenario, frame, and refresh rate produces the same RGB565
output.

Use `--settings-page-transition-demo` to capture the Settings page-depth
transition. Add `--navigation-transition-demo-reverse` to capture the backing
motion instead.

An explicit card-mode `screenshot-screensaver` capture opens the production
archive with a fixed smoke-test seed, advances the same renderer with the fixed
capture clock, and waits for a bounded number of production scaler results
before writing the image. It fails when the pack or production renderer is
unavailable; it never substitutes fixture screenshots or starts a download.

## What the preview exercises

- compiled HDMI and CRT Slint layouts, fonts, models, overlays, and animations;
- final 960x540 RGB565 composition;
- the production launcher hierarchy, navigation, velocity, and spring motion;
- the production Rust Arcade list renderer;
- the production screenshot scaling and crossfade compositor;
- the production particle renderer;
- the production archive-streamed, multi-depth screenshot parade with real
  Arcade pack images in card mode;
- mounted-card discovery and canonical `/media/fat` path mapping;
- the production Catalog V3 scanner, publisher, and loader with Mac-local state;
- the production preview archive resolver, RGB565 decoder, and media downloader.

The adapter supplies keyboard state, Mac storage roots, native window
presentation, and refresh timing. Fixture mode derives every system shell from
the canonical taxonomy and populates only Arcade. Card mode uses the real
catalog and screenshot packs. Launcher lifecycle, settings mutation, scanning,
catalog projection/loading, media download/decode, Slint presentation, RGB565
composition, transitions, and screensavers are shared with MiSTer.

It does not validate FPGA routing, HDMI/CRT scanout, vblank latch behaviour,
Linux controller mappings, Main handoff, or Cortex-A9 performance. Continue to
use normal device delivery and visual checks for those responsibilities.
