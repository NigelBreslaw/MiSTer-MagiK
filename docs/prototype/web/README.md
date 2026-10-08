# Web prototype (card launcher, system pages, CRT)

The HTML/JS prototype is the design reference for the card launcher, system
pages and native CRT layout. Its geometry, timing and ordering guide the
Rust/Slint implementation. Production input, responsive layouts, preparation
and presentation are owned by the [current architecture](../../architecture.md).
The completed port's implementation diary is retained in Git history.

| File | What it holds |
|---|---|
| `index.html` | Page shell, HDMI Settings (cog zoom, drop-downs, pages), key routing, hash review states |
| `browse.js` | HDMI launcher row, nested card levels (left row, step, cycling), the level trick, collection data |
| `gamelist.js` | HDMI system page: hub + game list with a generic device, Select toggle, card→page zoom, placeholder screenshots |
| `hub.js` | Shared hub panel (identity, name, count, Games / Recent / Favourites tiles) |
| `arcade.js` | HDMI Arcade page: cabinet, list, search, Arcade hub + Select toggle, Arcade card zoom |
| `crt.js` | The whole 640x240 CRT version (canvas), including card levels, trick, page, Arcade hub |
| `ask.js`, `about.html` | Dialogs and the About page |

## Assets (not checked in)

The code loads these from its own folder. They are either private, captures
of the device, or generated, so they are not copied here:

- Fonts: `nocive.ttf` and `xerxes.ttf` (`private/magik-assets/fonts/...`),
  `jersey25.ttf` (`apps/mister/ui/fonts/Jersey25-Regular.ttf`),
  `spleen-6x12.bdf` (`apps/mister/assets/fonts/spleen/`).
- Launcher card art `lc-0N_*.png`: `magick -size 360x504 -depth 8 rgb:<card>.rgb888`
  over `apps/mister/assets/ui/launcher-cards/*.rgb888`.
- Generic devices `device-{tv,monitor,handheld}.png`: `DEVICE_ART_DUMP=<dir>
  scripts/cargo test --features ui --lib dump_device_art -- --ignored`, then
  convert the PPMs.
- Arcade cabinet and card renders: `outputs/card-studio/renders/arcade-cabinet/`.
- Settings cog: `outputs/card-studio/renders/settings-backdrop/`.
- Device captures (`launcher-settings.png`, `launcher-arcade*.png`,
  `crt-launcher*.png`, `shot-720.png`, `shot-actionfighter.png`): captured from
  a MiSTer running MagiK.

Serve the folder with any static server (`python3 -m http.server`) and open
`index.html`. **Tab** is Select. Hash review states (for example `#slide-r-250`,
`#trick-460`, `#glist-600-h`, `#crt-pgat-450-h`) render a single frame; they are
listed below.

## Reference geometry and timing

The root retains production's cyclic carousel, artwork, spring and input
policy. Nested rows cycle from two cards; their selected card sits at the left
and the following cards overlap and recede. The front card clips during a step,
and the new end card turns behind the previous card with a linear angle clock.
The level trick turns and travels continuously between selected source and
destination slots; it does not spin a stationary hero and replace the level.

HDMI (logical 960×540, card 180×252 at scale 1, row centred on y = 284):

| Slot | Scale | Width × height | Left | Centre x | Brightness | Turn |
|---|---|---|---|---|---|---|
| 0 | 1.000 | 180.0 × 252.0 | 292.0 | 382.0 | 1.00 | 0° |
| 1 | 0.900 | 162.0 × 226.8 | 445.0 | 526.0 | 0.72 | 14° |
| 2 | 0.810 | 145.8 × 204.1 | 582.7 | 655.6 | 0.56 | 14° |
| 3 | 0.729 | 131.2 × 183.7 | 706.6 | 772.2 | 0.42 | 14° |
| 4 | 0.656 | 118.1 × 165.3 | 818.2 | 877.2 | 0.32 | 14° |
| hidden left (`goneLeft`) | 0.98 | — | — | 370 | — | opacity 0 |
| hidden right (`goneRight`) | 0.34 | — | — | ≈907 | 0.25 | opacity 0 |

`left(k+1) = left(k) + 0.85 × width(k)`. Slot 0's reference left edge (292) sits
27 px inside the sidebar rule at x = 265. Production derives clipping from
the native row layout.

CRT (640×240 raster, card 160 × 112 raster lines at scale 1, row centred on
line 110, left edge at the page margin `L = 38`):

| Slot | Scale | Width | Left | Centre x | Darkening |
|---|---|---|---|---|---|
| 0 | 1.000 | 160.0 | 38.0 | 118.0 | 0 |
| 1 | 0.900 | 144.0 | 174.0 | 246.0 | 0.40 |
| 2 | 0.810 | 129.6 | 296.4 | 361.2 | 0.55 |
| 3 | 0.729 | 116.6 | 406.6 | 464.9 | 0.65 |
| 4 | 0.656 | 105.0 | 505.7 | 558.2 | 0.72 |

CRT cards do not turn at rest (the prototype has no tilt on CRT). In
production, derive the CRT card size from `responsive::Layout` (`card_w`,
`card_h`, `margin_x`, `centre_y`) and keep the ratios above, rather than
hard-coding 160.

| Motion | Production duration | Reference windows |
| --- | ---: | --- |
| Nested row step | 460 ms | Shared cubic in-out position; linear end-card turn |
| Level trick | 920 ms | Gather 0–460; deal from 460 + 20·i for 360 ms; chrome out 0–260, swap 414, in 506–866 |
| HDMI card → page | 1000 ms | Window 0–760; face 60–260; device 80–840; hub from 500 + 26·i for 280 ms; screen 760–960 |
| CRT card → page | 900 ms | Window 0–680; face 30–230; snapshot 100–360; scrim 420–720; hub rows from 500 + 22·i for 260 ms |
| HDMI hub → list | 426 ms | Out 0–180; two incoming bands from 120 + 26·i for 280 ms |
| HDMI list → hub | 556 ms | Out 0–180; seven incoming bands from 120 + 26·i for 280 ms |
| CRT hub ↔ list | 340 ms | Out 0–150; swap 150; incoming bands from 150 + 14·i for 190 ms |

Production timing authorities are
[`launcher_navigation.rs`](../../../crates/framebuffer-scenes/src/launcher_navigation.rs),
[`launcher/level_trick.rs`](../../../crates/framebuffer-scenes/src/launcher/level_trick.rs),
[`device_card.rs`](../../../crates/framebuffer-scenes/src/device_card.rs) and
[`system_panel.rs`](../../../crates/framebuffer-scenes/src/system_panel.rs).
The panel renderer retains the prototype's CSS Bezier curves. Readiness holds
are separate from nominal motion time; destination preparation cannot consume
the reveal clock. Reduce motion settles immediately, and ordinary nested taps
during an active step remain discarded.

## System-page contract and review

One system page owns hub/list mode. Card entry, including Arcade, lands on the
hub; Select toggles mode, Games/Recent/Favourites select real sections, and Back
restores the source menu and selection. Direct entries and launch returns keep
their route-specific mode. Device art or the CRT screenshot stays stationary
under the panel bands. Reveal endpoints equal the source and settled page,
including reverse playback, clipping, backdrop and final highlight pixels.

Production hub titles use registered Jersey bitmap resources at 48/64 source
pixels, with counts at 52. The count/caption follows the measured title height;
HDMI reveal/panel bands include its region through y=302. Computer and handheld
icons use the two-tone 16×10 body/glass masks. Native CRT labels are
TITLES/PLAYED/SAVED with chevrons and an accent focus gradient. The settled CRT
backdrop is uniformly 40% brightness; the prototype's gradient scrim remains a
styling difference. Media, controller-port and core fact rows require real
metadata and remain deferred.

Geometry and typography outside landscape HDMI and native 640×240 CRT use
production responsive layouts. Portrait and other CRT profiles need their own
visual review; the prototype does not specify those layouts.

Open `index.html` with a hash. A trailing `-3` or `-4` picks
Computers or Handhelds where noted.

| State | Shows |
|---|---|
| `#consoles`, `#nintendo` | Settled rows (6 makers, 4 systems) |
| `#slide-r-<ms>`, `#slide-l-<ms>` [`-<maker>`] | Row step frames (maker 0 = Atari, 3 = Nintendo) |
| `#trick-<ms>` [`-<root index>`] | Level trick into a collection |
| `#back-<ms>` | Level trick back to the root |
| `#lvl-3`, `#lvl-4` | Computers / Handhelds row |
| `#glist-<ms>-h` / `-l` [`-<root index>`] | HDMI system page reveal frame, hub or list |
| `#archub`, `#arcade` | HDMI Arcade hub / list |
| `#crt-consoles`, `#crt-nintendo`, `#crt-lvl-3` | CRT rows |
| `#crt-stepat-r-<ms>` / `-l-` [`-<maker>`] | CRT step frames |
| `#crt-trickat-<ms>` [`-<root index>`] | CRT trick |
| `#crt-pg-h` / `-l` [`-<root index>`], `#crt-pgat-<ms>-h` | CRT page and its reveal |
| `#crt-archub` | CRT Arcade hub |

Key flows to reproduce on the device (Tab = Select in the browser):
launcher → Consoles → Nintendo → A on SNES → hub → Select → list → Select →
hub → Right, A (Recent list) → B (back to Nintendo); launcher → Arcade → hub →
Select → list → Y search → B → B (launcher).

Host raster, golden-frame and gesture checks establish rendering and navigation
contracts. They do not qualify physical output or cadence. Qualification must
identify the exact artifact, exercise entry/Select/sections/Back for Arcade,
Consoles, Computers and Handhelds on HDMI and CRT, and retain authoritative
FPGA-latched captures plus Analytics. Physical held-controller and cold-start
journeys remain separate from injected development input. The target is zero
dropped frames at the intended physical refresh rate, including steps, tricks,
reveals and toggles; host checks or an old device pass cannot waive it.
