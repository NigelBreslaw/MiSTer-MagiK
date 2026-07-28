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
apps/mister/scripts/dev-ui-mac.sh --scenario screenshot-tiles
apps/mister/scripts/dev-ui-mac.sh --scenario particle
```

Interactive previews use the current monitor refresh rate, capped at 120 Hz.
Force a target when comparing motion:

```bash
apps/mister/scripts/dev-ui-mac.sh --refresh-rate 60
apps/mister/scripts/dev-ui-mac.sh --refresh-rate 120
```

With `--refresh-rate auto` (the default), moving the window to another monitor
updates the target shown in the window title. An unfocused preview releases held
input and pauses its clock.

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
| `T` | Production screenshot-tile wall |

Arrow-key presses and releases drive the production launcher navigation,
including velocity, turbo re-press, and spring settlement. `Space` pauses a
screensaver; `.` advances one refresh interval while paused. On Home, `Up`
focuses the Settings gear and `Enter` opens it; `Down` returns to the system
tiles. `Enter` opens systems and supported subpages, including Settings →
Screensaver → Preview. Screenshot tiles return to the underlying launcher view
on input. `Escape` or `Backspace` goes back. The number shortcuts also work on
the numeric keypad.

## Deterministic captures

The capture path must not already exist:

```bash
apps/mister/scripts/dev-ui-mac.sh \
  --scenario arcade \
  --refresh-rate 120 \
  --frame 12 \
  --output /tmp/mister-magik-arcade.ppm
```

Useful capture scenarios include `home`, `arcade`, `settings`,
`arcade-crossfade`, `controller-setup`, `catalog-scan`, `particle`, and
`screenshot-tiles`. Captures use a fixed animation clock and deterministic
in-memory catalog/media fixtures. Headless `auto` uses 60 Hz; at an explicit
120 Hz, frame 12 is exactly 100 ms. Repeating a scenario, frame, and refresh
rate produces the same RGB565 output.

## What the preview exercises

- compiled HDMI Slint layouts, fonts, models, overlays, and animations;
- final 960x540 RGB565 composition;
- the production launcher hierarchy, navigation, velocity, and spring motion;
- the production Rust Arcade list renderer;
- the production screenshot scaling and crossfade compositor;
- the production particle renderer;
- the production time-based screenshot-tile wall algorithm.

The adapter supplies keyboard state, deterministic fixture media, native window
presentation, and refresh timing. It derives every system shell from the
canonical taxonomy and populates only Arcade, so launcher UI and navigation
changes are shared with MiSTer rather than reimplemented for macOS.

It does not validate FPGA routing, HDMI/CRT scanout, vblank latch behaviour,
Linux controller mappings, Main handoff, or Cortex-A9 performance. Continue to
use normal device delivery and visual checks for those responsibilities.
