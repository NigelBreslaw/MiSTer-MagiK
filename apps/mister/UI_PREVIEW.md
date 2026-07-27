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

Arrow keys change selection. `Space` pauses a screensaver; `.` advances one
frame while paused. On Home, `Up` focuses the Settings gear and `Enter` opens
it; `Down` returns to the system tiles. `Enter` opens supported subpages and
`Escape` or `Backspace` goes back. The number shortcuts also work on the
numeric keypad.

## Deterministic captures

The capture path must not already exist:

```bash
apps/mister/scripts/dev-ui-mac.sh \
  --scenario arcade \
  --frame 12 \
  --output /tmp/mister-magik-arcade.ppm
```

Useful capture scenarios include `home`, `arcade`, `settings`,
`controller-setup`, `catalog-scan`, `particle`, and `screenshot-tiles`.
Captures use a fixed animation clock and deterministic synthetic catalog/media
fixtures.

## What the preview exercises

- compiled HDMI Slint layouts, fonts, models, overlays, and animations;
- final 960x540 RGB565 composition;
- the production Rust Arcade list renderer;
- the production static screenshot scaling path;
- the production particle renderer;
- the production screenshot-tile wall algorithm.

It does not validate FPGA routing, HDMI/CRT scanout, vblank latch behaviour,
Linux controller mappings, Main handoff, or Cortex-A9 performance. Continue to
use normal device delivery and visual checks for those responsibilities.
