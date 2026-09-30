# Card launcher and system pages: web prototype → production parity plan

Status: implementation contract, 2026-10-01. Scope: HDMI (960×540 logical, landscape) and CRT
(640×240 raster, 2:1 pixel aspect). Portrait layouts are called out where the
prototype is silent.

The web prototype in [`prototype/web/`](prototype/web/README.md) is the
specification. Every section below names the prototype code that defines the
behaviour (file and line, which match that folder), the production Rust or
Slint that must change, the exact numbers to carry over, and how to prove it.
Production must reproduce the prototype's *geometry, timing and ordering*; it
does not need to reproduce its implementation (DOM transforms, canvas).

---

## Contents

1. [What production is missing](#1-what-production-is-missing)
2. [Ground rules for the port](#2-ground-rules-for-the-port)
3. [WS1 Nested-level card row: left-anchored, overlapping, cycling](#ws1-nested-level-card-row)
4. [WS2 Stepping a nested level: slide, clip, end-card flip](#ws2-stepping-a-nested-level)
5. [WS3 Card backs and collection icons](#ws3-card-backs-and-collection-icons)
6. [WS4 Level trick: continuous turn and travel](#ws4-level-trick-continuous-turn-and-travel)
7. [WS5 One page per system: hub + game list, Select toggles](#ws5-one-page-per-system)
8. [WS6 Full-screen reveals into the page](#ws6-full-screen-reveals-into-the-page)
9. [WS7 Hub ↔ list toggle motion](#ws7-hub--list-toggle-motion)
10. [WS8 Verification, baselines and performance](#ws8-verification-baselines-and-performance)
11. [Sequencing and PR slices](#11-sequencing-and-pr-slices)
12. [Open decisions and risks](#12-open-decisions-and-risks)
13. [Appendix A: prototype code index](#appendix-a-prototype-code-index)
14. [Appendix B: timing tables](#appendix-b-timing-tables)
15. [Appendix C: review states](#appendix-c-review-states)

---

## 1. What production is missing

What you see on the device today, set against the prototype:

| # | Symptom on the device | Prototype behaviour | Where production does it today | WS |
|---|---|---|---|---|
| 1 | Nested levels (Consoles, makers, systems) show the root's **centred fan** | Selected card at the **left**, the rest recede to the right, each 10% smaller and overlapping the one in front by 15% | `slot_geometry` / `continuous_geometry` in [`launcher.rs:786`](../crates/framebuffer-scenes/src/launcher.rs#L786), [`:1070`](../crates/framebuffer-scenes/src/launcher.rs#L1070); CRT via `map_plan` [`responsive.rs:414`](../crates/framebuffer-scenes/src/launcher/responsive.rs#L414) | 1 |
| 2 | Levels with fewer than 5 cards stop at their ends | Every level with **2 or more** cards cycles forever | `CYCLIC_LEVEL_MIN_CARDS = 5` [`launcher.rs:77`](../crates/framebuffer-scenes/src/launcher.rs#L77), used in the app at [`launcher.rs:1389`](../apps/mister/src/launcher.rs#L1389) | 1 |
| 3 | Stepping: end cards turn 150° at the far edges, the selection area is never clipped | Front card **slides left and is clipped**; the new end card **turns 180° out from behind** the previous card; end card always furthest back | slide branch of `build_carousel_plan` [`launcher.rs:1132`](../crates/framebuffer-scenes/src/launcher.rs#L1132), `SLIDE_FLIP` [`:1115`](../crates/framebuffer-scenes/src/launcher.rs#L1115) | 2 |
| 4 | The MagiK card back is almost never seen | Back is on screen for the first half of every end-card turn | `back_surface` [`artwork.rs:102`](../crates/framebuffer-scenes/src/launcher/artwork.rs#L102) exists but is only drawn past 90° of a 150° edge flip | 2, 3 |
| 5 | Computer and handheld card icons are the old single-tone bitmaps | 5:4 monitor and Game Boy–style handheld with a second **glass** tone | `category_icon` [`artwork.rs:400`](../crates/framebuffer-scenes/src/launcher/artwork.rs#L400) | 3 |
| 6 | Level trick: the chosen card spins **in place**, then the new level appears around it | The chosen card **turns 180° and travels** (centre ↔ left slot) for the whole 920 ms; edge-on and half way at 460 ms | [`level_trick.rs`](../crates/framebuffer-scenes/src/launcher/level_trick.rs) (`EDGE_MILLIS = 400`, fixed centre pose) | 4 |
| 7 | A system opens a **separate hub page**; its game list is another page | **One page**: device on the right, left side is the hub *or* the list, **Select** toggles, lands on the hub | `Screen::SystemHub` + `Screen::Arcade` [`launcher.rs:303`](../apps/mister/src/launcher.rs#L303), [`views/hdmi/system_hub.slint`](../apps/mister/ui/views/hdmi/system_hub.slint), [`views/hdmi/arcade_list.slint`](../apps/mister/ui/views/hdmi/arcade_list.slint) | 5 |
| 8 | Arcade has no hub (Games / Recent / Favourites) | Arcade page has the same hub, lands on it | Arcade opens straight to its list | 5 |
| 9 | Opening a system uses the generic scaler; hub → list has no motion | **Card → page zoom**: card outline zooms past the edges, the device rises out of the card, the hub deals in (HDMI); the screenshot grows from the card (CRT) | `NavigationTransitionEdge::ConsolesToSystem` → `SuperScaler` [`navigation.rs:38`](../crates/framebuffer-scenes/src/navigation.rs#L38), chosen in `navigation_transition_for_intent` [`launcher_loop.rs:1009`](../apps/mister/src/ui_runner/launcher_loop.rs#L1009) | 6 |
| 10 | Arcade card reveal lands on the list | Reveal lands on the **hub** | `render_arcade_card_transition_into` [`arcade_card.rs:52`](../crates/framebuffer-scenes/src/arcade_card.rs#L52) deals list bands | 6 |
| 11 | No hub ↔ list motion | Band push: one side slides out, the other deals in, device/screenshot never moves | — | 7 |

The **root row** (Settings, Arcade, Consoles, Computers, Handhelds, Favourites)
is already right in production and stays as it is. The prototype's root clamps
at its ends and its CRT root is a synthesised row; production's cycling root
and real CRT launcher win (see [§12](#12-open-decisions-and-risks)).

---

## 2. Ground rules for the port

- **Frames are pure functions of time.** Every prototype animation is a
  `frame(t)` with `t` in milliseconds; reverse plays the same function
  backwards. Keep that shape: it is what the existing renderers do
  (`render_level_gather/deal`, `render_arcade_card_transition_into`,
  `settings_cog`), and it is what makes review frames and tests exact.
- **Easing and windows are shared.** The prototype uses three helpers
  everywhere ([`browse.js:506-512`](prototype/web/browse.js#L506)):
  `inOut` (cubic in-out), `out` (quartic out, `1-(1-t)^4`), and
  `win(t, at, dur) = clamp01((t-at)/dur)`. Production already has Q16 forms in
  [`level_trick.rs:286-307`](../crates/framebuffer-scenes/src/launcher/level_trick.rs#L286)
  (`window`, `ease_in_out_cubic`, `ease_out_quart`). **Reuse the existing `card_page` helpers where their curves match.**
  Preserve the root carousel and renderer. The end-card angle uses linear
  elapsed time; HDMI band toggles use the explicit CSS Bezier curves in
  `gamelist.js`, not cubic/quartic substitutes.
- **Allocation-free in motion.** Faces, backs, device art and page rasters are
  prepared before a motion starts or on workers (as `LauncherCardHomeSession`
  does with `spawn_prepare`/`prefetch`, and as `SettingsCogRenderAhead` does).
- **RGB565, dithered.** Colours in the prototype are sRGB hex; production
  converts with the existing ordered-dither helpers (`lit_body`,
  `quantise_native`).
- **Fonts.** The Slint font contract applies (`Nocive15`, `Xerxes10`,
  `Jersey25` at `px25`/`px56`, `Spleen6x12`, `CrtLauncherText`; no raw `Text`
  or `font-size`). Where the prototype uses a size production does not have,
  this plan names the substitute.
- **Reduce motion.** Every new motion needs a reduce-motion path. The
  prototype does not model it; the rule is below in each workstream.

---

## WS1 Nested-level card row

### Behaviour

- The **root** keeps its centred fan. **Every level below it** uses a row
  anchored to the left edge of the carousel area.
- Five visible slots. Slot *k* has scale `0.9^k` (100%, 90%, 81%, 73%, 66%).
- Each card starts **85% of the way across** the card in front of it, so 15%
  of every card is covered by the next nearer card.
- Cards darken with depth. On HDMI, cards after the first are turned 14°
  (the right edge recedes).
- Levels with **2 or more** cards cycle forever. One card: static.
- A level with fewer cards than slots uses only the first `min(n, 5)` slots.

### Prototype code

- HDMI slot table, hidden slots and cycling:
  [`browse.js:257-278`](prototype/web/browse.js#L257):
  `at()` (screen-exact placement with perspective undone),
  `SHRINK`, `STEP`, `LS`, `LB`, `LEFT_EDGE`, `TILT`, `LEFT`, `K`,
  `CYCLE_MIN`, `goneLeft`/`goneRight`, `relOf`, `slotFor`.
- Resting layout: `layoutCards` [`browse.js:293`](prototype/web/browse.js#L293).
- Root fan (unchanged): `slot` [`browse.js:245`](prototype/web/browse.js#L245).
- CRT row: [`crt.js:636-643`](prototype/web/crt.js#L636) (`BSHRINK`, `BSTEP`,
  `BSS`, `BDARK`, `BSLOTS`, `BGONE_L`, `BGONE_R`, `bcyc`, `brel`, `bslotOf`),
  `drawCarousel` [`crt.js:697`](prototype/web/crt.js#L697), root fan
  `rslot` [`crt.js:687`](prototype/web/crt.js#L687).

### Exact geometry

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

`left(k+1) = left(k) + 0.85 × width(k)`. Slot 0's left edge (292) sits 27 px
inside the sidebar rule at x = 265, which is where production's carousel clip
already starts (`clip: (296, 934)` in `continuous_geometry`).

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

### Production changes

1. **Level layout kind.** Add a layout selector to the prepared launcher,
   derived from `LauncherLevel`: `Root` keeps `slot_geometry`/`slot_angle`;
   `Nested` uses a new `row_slot(k)` table that returns the poses above.
   Keep one table for HDMI logical coordinates and let `map_plan` map CRT
   (or give CRT its own native table, which the prototype effectively does).
   - [`launcher.rs:786`](../crates/framebuffer-scenes/src/launcher.rs#L786)
     `slot_geometry` → becomes the root case.
   - [`launcher.rs:1070`](../crates/framebuffer-scenes/src/launcher.rs#L1070)
     `continuous_geometry` → dispatch on layout kind.
   - [`responsive.rs:414`](../crates/framebuffer-scenes/src/launcher/responsive.rs#L414)
     `map_plan` currently maps the centred fan with a distance curve around
     x = 610. The row needs a left-anchored mapping (margin + ratios), not the
     centre curve.
2. **Brightness per slot** (0.72 / 0.56 / 0.42 / 0.32) on HDMI and a darkening
   overlay on CRT. Check whether `launcher_flip::draw` already takes a light
   factor (`diffuse_light`); if not, add a per-item brightness to
   `CarouselItem`.
3. **Draw order and occlusion.** The row overlaps: draw far → near (slot 4
   first) or near → far with `BodyOcclusion` so each nearer card hides the
   left 15% of the next. The fan's `body_clip` trick in `build_carousel_plan`
   (which trims a side card behind its neighbour) does not apply to the row.
4. **Plan capacity.** `CarouselPlan::items` is `[Option<_>; 6]`
   ([`launcher.rs:1128`](../crates/framebuffer-scenes/src/launcher.rs#L1128)).
   A stepping row can show 5 settled cards + 1 leaving + 1 entering = 7.
   Raise it to 8 together with scratch buffers and occlusion storage.
5. **Cycling from 2 cards.** `CYCLIC_LEVEL_MIN_CARDS = 2`. The prototype's
   `relOf`/`slotFor` treat `n > K` specially: the card at relative `n-1` is
   parked on the hidden-left slot. For `n ≤ 5` every card is visible in slots
   `0..n-1`. The same card may legitimately appear twice during a step (WS2),
   so plans must allow duplicate face indices.
6. **App navigation.** `LauncherNav::set_wraps`
   ([`launcher.rs:525`](../apps/mister/src/launcher.rs#L525)) and the
   threshold at [`launcher.rs:1389`](../apps/mister/src/launcher.rs#L1389) move to
   the new minimum. `update_card_scroll`
   ([`launcher.rs:2977`](../apps/mister/src/launcher.rs#L2977)) is unchanged:
   Right advances the selection, and the row moves left.

### Acceptance

- Settled review frames for Consoles (6 makers), Nintendo (4 systems),
  Atari (3), a 2-card level and a 1-card level match the prototype's
  `#consoles`, `#nintendo`, `#slide-r-0-0` (Atari), within a few pixels of
  card edges, on HDMI and CRT (`#crt-consoles`, `#crt-nintendo`).
- Portrait: see [§12](#12-open-decisions-and-risks).

---

## WS2 Stepping a nested level

### Behaviour (Right; Left is the same run backwards)

Duration 460 ms. With `k = inOut(t/460)`:

- **Middle cards** move one slot left together, interpolating position, scale,
  brightness and turn.
- **Front card** does **not** spin. It slides left to a slot wholly past the
  clip edge (`slideOut`, centre x = 158, scale 1) and is **clipped** at
  `SLIDE_CLIP = LEFT_EDGE − 24 = 268` (HDMI), or at the page margin `L` (CRT).
  It is drawn on top.
- **End card** (the new last card) starts **tucked behind the previous card**
  (the end slot's pose shifted 10 px left on HDMI, 8 px on CRT) and moves to
  the end slot. It turns from **180° (showing its MagiK back) to face-on**,
  with the angle **linear in time** (`180 → 14°` over the full 460 ms; not
  eased). It is always drawn **furthest back**.
- **Left** plays the same frame backwards: the front card slides back in from
  the clip edge and the end card turns back to its MagiK back and tucks away.
  The leaving end card disappears at the end of its turn.
- **Short levels** (`n ≤ 5`): the card that slides out at the front is the
  same card that turns in at the end. The prototype clones it; production
  just draws the same face index in both roles.
- Only the root keeps its old flip-in-place selection.

### Prototype code

- HDMI: `slideLeft` [`browse.js:323-367`](prototype/web/browse.js#L323)
  (`SLIDE_MS`, `SLIDE_CLIP`, `slideOut`, `Ke = min(n, K)`, `leaveI`/`enterI`,
  clone, `tuck`, linear `p.ry`, z-order 30 / 20−rel / 1).
- CRT: `bstepRoles` [`crt.js:702`](prototype/web/crt.js#L702),
  `stepCards` [`crt.js:713`](prototype/web/crt.js#L713) (clip to `L`,
  end card drawn first, squeeze `sx = cos(angle)` with the back when
  `sx < 0`), `browseStep` [`crt.js:729`](prototype/web/crt.js#L729).

### Production changes

- Rewrite the slide branch of `build_carousel_plan`
  ([`launcher.rs:1132`](../crates/framebuffer-scenes/src/launcher.rs#L1132)):
  - Replace the `leaving`/`entering` 150° edge flip (`SLIDE_FLIP`) with the
    three roles above.
  - The front card needs a **per-item horizontal clip**. `Pose.clip` exists;
    intersect each item clip with the render strip, instead of replacing
    it in `draw_carousel_plan`; use 268 as the nested step clip edge.
    Nested settled bounds must also admit the slot-0 edge at 292.
  - The end card's angle is `lerp(π, tilt, t / 460)` using the *linear* time
    fraction, not the spring/eased progress. The back face shows while
    `cos(angle) < 0` (the existing `extra.abs() > GEOMETRY_ONE / 2` test,
    re-expressed on the absolute angle).
- **Duration and easing.** Production browse motion uses the shared spring
  (`smooth_progress`). The prototype uses a fixed 460 ms cubic in-out. Use
  fixed 460 ms cubic motion for nested steps (see §12); keep root spring
  motion unchanged. Carry actual elapsed milliseconds separately from
  integrated spring position; the end-card turn stays linear in *time*.
- **Hold/repeat.** Held D-pad repeats in production queue steps; each step
  replays this motion. Confirm `is_visually_at_rest` still waits for the end
  card to finish turning before A acts.
- CRT has no 3D turn: draw the squeeze exactly as the prototype does
  (`|cos|` horizontal scale about the card centre, back image when negative).
  In production, CRT already goes through the same flip rasteriser; check that
  its CRT faces include a back (WS3).

### Acceptance

Review frames `#slide-r-100/200/300/400`, `#slide-l-150/300`, Atari
`#slide-r-200-0`, CRT `#crt-stepat-r-150/210/300`, `#crt-stepat-l-200`. The
back must be visible behind the previous card at around 200 ms, and face-on by
around 350 ms.

---

## WS3 Card backs and collection icons

### Backs

- Every **generic** card (every card below the root) has a MagiK back in its
  **collection colour**: dark tinted base, diagonal crosshatch, inset frame,
  diamond with the cream **M**.
- HDMI: `backImage` [`browse.js:205`](prototype/web/browse.js#L205).
  Production already has this as `back_surface`
  ([`artwork.rs:102`](../crates/framebuffer-scenes/src/launcher/artwork.rs#L102),
  gated by `has_back` [`:32`](../crates/framebuffer-scenes/src/launcher/artwork.rs#L32)).
  It is correct in design; WS2 is what makes it visible. Verify it follows
  `card.colour` for Computers (yellow) and Handhelds (green), not only
  Consoles blue.
- CRT: `crtBack` [`crt.js:659`](prototype/web/crt.js#L659) is the same design
  at 160 × 112 raster (half-height lines: shapes are drawn at `scale(1, .5)`,
  the M is a 12-line Spleen glyph). Production must bake a native CRT back at
  the CRT card size with the same half-height correction the CRT faces use.
- Root cards with artwork do not need a back: they never show one (the level
  trick shows the next level's face on the far side).

### Collection icons (two-tone)

- Icons are 16 × 10 cells with three values: `0` empty, `1` solid body
  (cream), `2` **glass** (the collection colour darkened:
  `mix(#05070c, accent, 0.5)`).
- Designs: `GAMEPAD`, `COMPUTER` (5:4 monitor: 10 × 8 body, 8 × 6 glass, neck,
  base) and `HANDHELD` (Game Boy–style: glass screen, d-pad cut-out, buttons)
  at [`browse.js:59-66`](prototype/web/browse.js#L59). Consoles keep the
  gamepad exactly as it is.
- Card drawing with the glass tone: `cardImage`
  [`browse.js:75`](prototype/web/browse.js#L75) (5 px cells, 3 px drop in the
  deep colour, glass only on the face layer). CRT card: `crtCard`
  [`crt.js:610`](prototype/web/crt.js#L610) (4 × 2 raster per cell). The same
  bitmaps drive the system-page watermark on CRT (`drawIcon`,
  [`crt.js:981`](prototype/web/crt.js#L981), glass at 45% alpha).
- Production: replace the arrays in `category_icon`
  ([`artwork.rs:400-455`](../crates/framebuffer-scenes/src/launcher/artwork.rs#L400))
  with the prototype's, add the glass tone to the icon painter, and keep
  `faces_have_no_ordinal_dots_or_top_dash`-style tests green.
- The HDMI prototype's big voxel icon on the old system page (`heroImage`,
  [`browse.js:441`](prototype/web/browse.js#L441)) is **retired** by WS5: the
  page's hero is the device.

---

## WS4 Level trick: continuous turn and travel

### Behaviour (T = 920 ms, EDGE = T/2 = 460 ms)

- **The chosen card** turns `180° × inOut(t/T)` and travels
  `lerp(from_slot0, to_slot0, inOut(t/T))` for the **whole** trick. `from` and
  `to` are slot 0 of the level being left and the level being entered: the
  root's centre (x = 610) or the row's slot 0 (x = 382). At EDGE it is
  edge-on and half way. It lifts slightly: scale `1 + 0.04 sin(π t/T)`. Its
  face is the level being left until 90°, then the level being entered.
- **Gather (0 → EDGE):** the other cards of the level being left move from
  their slots to a point *behind the moving card* (0.9 × size, dark), with
  `inOut(win(t, 0, EDGE))`, turning edge-on as `90° × e²`, and vanish in the
  last 20 ms before EDGE.
- **Deal (EDGE → T):** the new level's cards start behind the moving card and
  slide out to their slots with `out(win(t, EDGE + min(n,5)·20, T − EDGE − 100))`
  (nearest first), turning from edge-on to face-on (`−90°·(1−k)` plus their
  resting tilt). Visible from EDGE onwards.
- **Chrome:** the header/sidebar fade and slide out over 0–260 ms
  (`inOut`), swap at 0.45 T, and slide in from 0.55 T over 360 ms (`out`).
- **Reverse** (B) is the same function with the levels swapped.

### Prototype code

`trick` [`browse.js:373-432`](prototype/web/browse.js#L373) (hero pose,
`behindOld`, gather, deal, chrome), CRT `trickState` / `browseTrickFrame`
[`crt.js:760-809`](prototype/web/crt.js#L760) (squeeze instead of turn).

### Production today and what changes

[`level_trick.rs`](../crates/framebuffer-scenes/src/launcher/level_trick.rs)
has the same two-half structure but a different motion: `EDGE_MILLIS = 400`,
`SNAP_MILLIS = 200` (the card snaps round after edge-on), the chosen card stays
at the centre (`scaled_centre`), and the deal comes out around the centre.

1. `LEVEL_TRICK_EDGE_MILLIS = 460`; remove `SNAP_MILLIS`; the chosen card's
   angle is `π·inOut(t/T)` throughout.
2. **Hero travel across the two halves.** The gather is rendered by the
   level being left and the deal by the level being entered, each by its own
   `PreparedLauncher`. Both must compute the same hero pose at every `t`. Give
   `render_level_gather` / `render_level_deal` the *other* level's slot-0 pose
   (or its layout kind: root fan or row), for example as fields on
   `LevelChange`, and replace `scaled_centre(lift(t))` with
   `hero_pose(t, from, to)`.
3. Gather target: `lerp_pose(rest, behind(hero_pose(t)), gather)`.
   Deal start: `behind(hero_pose(t))`.
4. Deal windows: stagger 20 ms (was 30), duration `T − EDGE − 100 = 360`
   (was 480), capped at 5 steps.
5. Chrome windows as above (`CHROME_IN_AT_MILLIS` moves to `0.55 T`).
6. **Holding at the edge.** `LauncherCardHomeSession` holds every card
   edge-on at EDGE until the destination level is prepared
   ([`launcher_card_home.rs:471`](../apps/mister/src/ui_runner/launcher_card_home.rs#L471)
   `render_trick`, [`:413`](../apps/mister/src/ui_runner/launcher_card_home.rs#L413)
   `take_built_destination`). With a travelling card, the hold freezes it half
   way; that is acceptable, and prefetch (`prefetch`,
   [`:360`](../apps/mister/src/ui_runner/launcher_card_home.rs#L360)) makes it
   rare. Keep the existing "no hold after prefetch" tests.
7. Reduce motion: unchanged (level changes without the trick).

### Acceptance

`#trick-150/300/460/580/740/920` (into Consoles), `#trick-300-3` and
`#trick-700-4` (Computers, Handhelds), `#back-250/460/680/920` (back to the
root), CRT `#crt-trickat-200/380`, `#crt-trickat-300-4`, `#crt-trickat-700-3`.
Existing tests `trick_starts_on_the_source_and_ends_on_the_destination` and
`both_halves_meet_at_an_all_edge_on_hold_frame` still hold; add one that the
hero pose from gather and deal agree at every millisecond around EDGE.

---

## WS5 One page per system

### Behaviour

- Every system and **Arcade** has **one page**. The device is on the right:
  the cabinet for Arcade, a TV for consoles, a monitor for computers, a
  handheld for handhelds. Its screen shows the selected game's screenshot.
- The left side is either the **hub** or the **game list**. **Select**
  toggles between them. The page **lands on the hub**.
- **Hub:** identity line (`MAKER / YEAR / GENERATION`), the system's full
  name, `N GAMES READY TO PLAY`, three tiles Games / Recent / Favourites with
  counts, and a caption for the focused tile. Left/Right move between tiles
  (HDMI; Up/Down on CRT rows). **A** on a tile opens that section's list
  (filtered); a tile with 0 does nothing.
- **List:** the Arcade list layout for any system, filtered by section, label
  `SNES` / `SNES / RECENT` / `SNES / FAVOURITES`, count right-aligned.
- **B always leaves the page** (back to the level of cards, or to the
  launcher for Arcade), from either side.
- In the hub the device screen shows the list's current selection (the
  prototype uses the first games; production should use the last-played game,
  falling back to the first).
- Footers: hub `A OPEN · B BACK · SELECT GAME LIST`; list
  `A PLAY · B BACK · SELECT OVERVIEW` (Arcade list keeps `Y SEARCH`,
  `X OPTIONS`).

### Prototype code

- Shared hub panel: `HUB.build` [`hub.js:30-54`](prototype/web/hub.js#L30),
  styles [`hub.js:8-20`](prototype/web/hub.js#L8).
- HDMI system page: [`gamelist.js`](prototype/web/gamelist.js): geometry
  constants [`:16-22`](prototype/web/gamelist.js#L16), page build
  [`:102-145`](prototype/web/gamelist.js#L102), `fillList`
  [`:153`](prototype/web/gamelist.js#L153), `setup` (hub config, captions)
  [`:167`](prototype/web/gamelist.js#L167), `applyMode`
  [`:199`](prototype/web/gamelist.js#L199), `openSection`
  [`:247`](prototype/web/gamelist.js#L247), `key`
  [`:311`](prototype/web/gamelist.js#L311).
- HDMI Arcade hub: [`arcade.js:37-82`](prototype/web/arcade.js#L37)
  (`RECENT`, `FAVS`, `fillList`, `buildHub`, `setSection`, `applyAMode`,
  `toggleMode`, `openSection`) and `key` [`:310`](prototype/web/arcade.js#L310).
- CRT page: [`crt.js:833-972`](prototype/web/crt.js#L833) (`slOpenState`,
  `slRows`, `slChrome`, `slContent`, `slFootMode`, `drawSysPageStatic`,
  `slOpen`), hub rows `drawSystem`
  [`crt.js:988`](prototype/web/crt.js#L988) (identity header plus Games /
  Recent / Favourites, then Media / Controller ports / MiSTer core), CRT
  Arcade hub [`crt.js:207-266`](prototype/web/crt.js#L207) (`ARC_SYS` at
  [`:141`](prototype/web/crt.js#L141)), input `browseKey`
  [`crt.js:1053`](prototype/web/crt.js#L1053).
- Browser key for Select: **Tab** ([`index.html`](prototype/web/index.html),
  both `keydown` listeners).

### HDMI layout (960×540)

| Element | Value |
|---|---|
| Header band | black 0–77, `MISTER MAGIK` at (26, 21), clock at (874, 26), rule at y = 76 |
| Footer band | black 500–540, rule at y = 500, footer text at y = 518 |
| Device | 483 × 519 at (490, 35); screen 320 × 320 at (572, 96); header/footer bands crop it (same as today's Arcade) |
| Hub identity | x = 30, y = 104, muted |
| Hub name | x = 28, y = 126, width 440, Jersey 64 px (≤ 13 chars) else 48 px, line height 0.9 × size, wraps |
| Hub count | x = 30, y = name bottom + 10 |
| Tiles | 140 × 128 at x = 30 + 150 i, y = 318; radius 10; focused tile lifts 8 px, accent border, glow, 3 px accent underline |
| Caption | x = 30, y = 470 |
| List | as Arcade: x = 26, width 462, view 124–494, rows 36, focus row 3 |

Font substitutions: the hub name's 64/48 px Jersey maps to `Jersey25 px56`
(one line) and `px25`-based two-line fallback, or a dedicated px48 primitive
(decision in §12). Tile numbers use the existing 52 px Jersey tile style from
today's `system_hub.slint`.

### CRT layout (640×240)

Full-screen screenshot backdrop with the Arcade scrim (`drawBackdrop`,
`scrim`, [`crt.js:150-181`](prototype/web/crt.js#L150)); hub = `drawSystem`
rows; list = `slRows` (x 38–400, rows 16, focus row 3, accent focus
gradient). This is the production CRT Arcade list plus hub rows; the CRT
system hub's scaled device hero (`crt_hero_dims`) is retired.

### Production changes

**Navigation model** ([`apps/mister/src/launcher.rs`](../apps/mister/src/launcher.rs)):

- Merge `Screen::SystemHub` into the Arcade screen
  ([`:303`](../apps/mister/src/launcher.rs#L303)) and add a page mode
  `enum SystemPageMode { Hub, List }` on the nav state. Landing mode is `Hub`
  for every entry (system card, Arcade card, launch return lands where it
  left).
- `handle_system_hub` ([`:2759`](../apps/mister/src/launcher.rs#L2759))
  becomes the hub-mode input of the page: tiles move with Left/Right on
  HDMI landscape and Up/Down on HDMI portrait (today's rule), and Up/Down on
  CRT, where the prototype's hub is a column of rows (today's CRT hub is a
  row of tiles moved with Left/Right); A opens
  `ArcadeUserListMode::{Games, Recent, Favourites}`
  ([`:315`](../apps/mister/src/launcher.rs#L315)) and switches to `List`.
- **Select** (`LogicalAction::Select` → `btn_select`,
  [`launcher.rs:5130`](../apps/mister/src/launcher.rs#L5130)) toggles the mode
  on this screen. Check the one existing use in
  [`launcher_loop.rs:4296`](../apps/mister/src/ui_runner/launcher_loop.rs#L4296)
  does not conflict.
- B leaves the page from either mode. `open_system_game_list`
  ([`:1731`](../apps/mister/src/launcher.rs#L1731)) and `skip_system_page`
  ([`:1723`](../apps/mister/src/launcher.rs#L1723)) keep direct entries
  (launch return, start-system setting, benchmarks) landing on the list.
- Arcade gets the hub: identity `MANY MAKERS / 1971-2005 / COIN-OP` (prototype
  text; confirm), counts from its existing Recent/Favourites lists.

**Slint** ([`ui/api.slint`](../apps/mister/ui/api.slint)):

- `NavigationView` / `ArcadeView` gain the page mode, the section, the hub
  counts (today's `system-hub-*-count`), identity strings (`system-title`,
  `system-subtitle`, `system-hub-caption` already exist), and the accent.
- Replace `views/hdmi/system_hub.slint` and `views/hdmi/arcade_list.slint`
  with one HDMI page that keeps the device/screen composition of
  `arcade_list.slint` and hosts both left panels; same for CRT
  (`views/crt/system_hub.slint` + `views/crt/arcade_list.slint`). Keep the
  list, search and drawer exactly as they are.
- The preview compositor keeps composing the screenshot into the device
  screen ([`ui_runner/preview_compositor.rs`](../apps/mister/src/ui_runner/preview_compositor.rs));
  in hub mode it composes the hub's representative game. CRT keeps
  `crt_backdrop_controller.rs`.

**Presenter** ([`launcher_presentation.rs`](../apps/mister/src/launcher_presentation.rs)):
`set_system_hero` / `crt_hero_dims` ([`:508`](../apps/mister/src/launcher_presentation.rs#L508))
go; `device_image` ([`:525`](../apps/mister/src/launcher_presentation.rs#L525))
already supplies the device backdrop.

### Acceptance

HDMI `#glist-1000-h`, `#glist-1000-l`, `#glist-1000-h-3`, `#glist-1000-h-4`,
`#archub`, `#arcade`; CRT `#crt-pg-h`, `#crt-pg-l`, `#crt-pg-h-3`,
`#crt-archub`. Key flows (Appendix C) end in the right mode with the right
filtered list. App tests: land on hub; Select toggles; A on 0-count tile is a
no-op; B from list leaves the page; direct entries land on the list.

---

## WS6 Full-screen reveals into the page

### 6a HDMI: system card → page (1000 ms)

From [`gamelist.js:254-283`](prototype/web/gamelist.js#L254) (`zoomFrame`):

- Source rect: the chosen card in row slot 0, `(292, 158, 180, 252)`.
- The **outline** (3 px accent, glow) scales about the card centre by
  `z = exp(ln(ZMAX)·inOut(win(t, 0, 760)))`, with
  `ZMAX = 1.15 · max(max(cx, 960−cx)/(w/2), max(cy, 540−cy)/(h/2))` so it
  always clears the screen. Opacity `max(0, 1 − 1.25·win(t,0,760)²)`.
- The **card face** scales with the window and fades over 60–260 ms.
- The **device rises out of the card**: its region `REGION = (117, 46,
  250 × 350)` (the screen and bezel, the card's aspect) starts exactly on the
  card, then translates/scales to its resting place `(490, 35)` with
  `inOut(win(t, 80, 760))`, **clipped to the growing rounded window** until
  the window passes `ZMAX`.
- The **level being left** fades over 40–280 ms; the page's header band fades
  in over 260–460 ms.
- The **hub bands** deal in from 500 ms, stagger 26 ms, 280 ms each, `out`,
  sliding 40 px from the right.
- The **screenshot** lights up over 760–960 ms; the footer over 640–860 ms.
- **B** plays the same frames backwards.

Production: `arcade_card.rs` already implements this pattern for the cabinet
(`render_hdmi`, [`arcade_card.rs:90`](../crates/framebuffer-scenes/src/arcade_card.rs#L90);
constants `HDMI_CARD`, `HDMI_CABINET_X/Y`, `HDMI_SCREEN`, `LIST_BANDS` at
[`:10-45`](../crates/framebuffer-scenes/src/arcade_card.rs#L10)).
Generalise it into a **device-card reveal** parameterised by the source card
rect, the device raster (cabinet or `device_art::device_rgb565`, both
483 × 519 with the same screen opening), the device region, the accent and
the destination bands (hub bands vs list bands). Wire it as a new
`NavigationTransitionRenderer` next to `ArcadeCard`
([`navigation.rs:80`](../crates/framebuffer-scenes/src/navigation.rs#L80)) and
use it for `ConsolesToSystem` in `navigation_transition_for_intent`
([`launcher_loop.rs:1009`](../apps/mister/src/ui_runner/launcher_loop.rs#L1009))
and at the `begin_arcade_card` call site
([`launcher_loop.rs:8251`](../apps/mister/src/ui_runner/launcher_loop.rs#L8251)).
The card rect comes from the card launcher (row slot 0), not the fan centre.

### 6b CRT: system card → page (900 ms)

From `slPageZoomFrame` [`crt.js:916-945`](prototype/web/crt.js#L916):

- The window interpolates from the card (`BCARD`, slot 0 of the CRT row) to
  the full raster with `inOut(win(t, 0, 680))`, corner radius 4 → 0.
- Inside the window: black, then the **screenshot drawn into the window**
  (`drawBackdrop(1, window)`), the scrim fading in over 420–720 ms, and the
  card face fading out over 30–230 ms.
- The level being left (a snapshot) fades over 100–360 ms; `MISTER MAGIK`
  fades in over 260–440 ms (so it never overprints the breadcrumb).
- Hub rows deal from 500 ms, stagger 22 ms, 260 ms, `out`, 24 px; footer
  560–760 ms.

Production: `arcade_card.rs` `render_crt`
([`:184`](../crates/framebuffer-scenes/src/arcade_card.rs#L184)) is the CRT
Arcade reveal; generalise the source card rect and the destination bands the
same way.

### 6c Arcade reveal lands on the hub

HDMI [`arcade.js:140-176`](prototype/web/arcade.js#L140) (hub branch: hub
bands deal with the same windows as 6a), CRT `arcZoomFrame`
[`crt.js:270-294`](prototype/web/crt.js#L270). Production: the destination
bands in `render_arcade_card_transition_into` become the hub bands when the
page lands on the hub.

### Reduce motion

A short crossfade (the prototype's CRT Settings zoom uses 260 ms under
reduce motion, [`crt.js:1155`](prototype/web/crt.js#L1155)) or an instant
switch; the device and screenshot appear in place. Follow whatever the
production Settings cog and Arcade reveals already do under reduce motion so
all card → page reveals behave alike.

---

## WS7 Hub ↔ list toggle motion

The device (HDMI) or screenshot (CRT) never moves; only the left panel
changes.

- **HDMI** (`pushBands` [`gamelist.js:228-246`](prototype/web/gamelist.js#L228),
  [`arcade.js:282-293`](prototype/web/arcade.js#L282)): the outgoing side's
  bands fade and slide 40 px left over 180 ms (`ease-in`); at 120 ms the
  incoming bands slide in from +32 px, 280 ms each, staggered 26 ms
  (`ease-out`). Direction flips going back.
- **CRT** (`slToggle` [`crt.js:898-909`](prototype/web/crt.js#L898),
  `arcToggle` [`crt.js:235-248`](prototype/web/crt.js#L235)): 340 ms; out
  0–150 ms (−24 px, fading); mode swaps at 150 ms; in from 150 + 14 ms·i,
  190 ms each (+24 px → 0).
- A on a tile = switch section, then run the same toggle.

Production: the segmented Settings page push already moves Slint's own
raster in bands with a stagger (`SettingsPageTransitionStyle::Segmented`,
[`navigation.rs`](../crates/framebuffer-scenes/src/navigation.rs)). Reuse that
renderer restricted to the left panel rect (x < 488 on HDMI; the list column
on CRT) with the timings above. Doing it in Slint animations is not
recommended: bands must move at whole-pixel offsets without re-rendering
text, which the segmented renderer already guarantees.

---

## WS8 Verification, baselines and performance

- **Review frames.** Every prototype review state in Appendix C gets a Rust
  counterpart that renders the same frame: extend
  [`examples/launcher_layout_review.rs`](../apps/mister/examples/launcher_layout_review.rs)
  (it already renders the trick and browse frames) and add reveal/toggle
  frames for the navigation renderers. Compare side by side, then commit
  baselines through the visual-baseline manifest.
- **Unit tests** (framebuffer-scenes): row slot table; plan item counts ≤ 8;
  duplicate face indices for short levels; end-card angle linear in time;
  front card clipped at 268; trick hero pose continuous across gather/deal;
  reveal endpoints exact (t = 0 is the source frame, t = end is the page) as
  `arcade_card::tests::endpoints_are_exact` does today.
- **App tests** (launcher.rs): cycling from 2 cards; hub landing; Select;
  section filters; B from both modes; direct entries.
- **Device.** `scripts/magik deploy` then a scripted journey: root → Consoles
  → Nintendo → SNES page → Select → list → B, and the same for Arcade,
  Computers and Handhelds, on HDMI and a CRT mode; capture frames
  mid-motion.
- **Performance.** Zero dropped frames at 60 Hz with Analytics on:
  - the row step (5–7 overlapping cards, one clipped, one turning);
  - the trick with travel;
  - both reveals (prepare page/device rasters before t = 0, as the cog and
    Arcade reveals do; allocation-free frames);
  - the band toggle.
  The overlapping row draws more columns than the fan; rely on
  `BodyOcclusion` so hidden columns are skipped.

---

## 11. Sequencing and PR slices

| Slice | Content | Depends on | Visible result |
|---|---|---|---|
| P1 | WS1 row geometry (HDMI + CRT), cycling from 2, plan capacity, settled frames | — | Cards in the right place |
| P2 | WS2 step motion + WS3 backs visible, CRT squeeze | P1 | Front slides/clips, end card turns from its back |
| P3 | WS3 two-tone icons | — (parallel) | Proper monitor and handheld |
| P4 | WS4 trick with travel | P1 | Chosen card flies centre ↔ left |
| P5 | WS5 page model + HDMI/CRT pages + Arcade hub + Select (no new motion; instant switch) | — (parallel with P1–P4) | One page, hub landing |
| P6 | WS7 band toggle | P5 | Hub ↔ list motion |
| P7 | WS6 reveals (device-card renderer, CRT window reveal, Arcade lands on hub) | P1, P5 | Full-screen card → page zoom |
| P8 | WS8 baselines, device journeys, frame-rate sign-off | all | Release |

P1 + P2 fix what is most visibly wrong today and are self-contained in the
framebuffer-scenes crate plus one app threshold.

---

## 12. Open decisions and risks

| Topic | Question | Default in this plan |
|---|---|---|
| Step timing | Keep production's spring (`smooth_progress`) or the prototype's fixed 460 ms cubic? | Fixed 460 ms cubic for nested steps; root keeps its existing spring; end-card turn uses real elapsed time |
| Root row | Prototype root clamps and its CRT root is synthesised | Production root (cycling, real CRT launcher) is authoritative |
| Portrait HDMI, CRT portrait, 288p/480p/5:4 CRT | Not prototyped | Row anchored to the top-left in portrait, ratios unchanged; confirm with a prototype pass |
| Hub name font | Prototype uses Jersey 64/48 px | `Jersey25 px56`, two lines allowed; add a px48 primitive only if needed |
| Hub device screen | Which game shows in hub mode? | Last played, else first |
| Arcade identity line | `MANY MAKERS / 1971-2005 / COIN-OP` is placeholder copy | Confirm wording |
| Hold at edge | The trick may hold half way while the next level prepares | Accept; prefetch keeps it rare |
| Select on controllers | Not every controller has a labelled Select | Controller setup maps it; footer says SELECT |
| CRT pixel budget | Overlapping row + squeeze on 640×240 | Measure in P2 |
| Placeholder art | Prototype screenshots are generated | Production uses real previews; never port `shotSrc` |

---

## Appendix A: prototype code index

All paths are under [`prototype/web/`](prototype/web/).

| Prototype | Lines | Purpose | Production target |
|---|---|---|---|
| `browse.js` `TREE`/`COMPUTERS`/`HANDHELDS`/`paint`/`ROOT` | 12–57 | Demo data, collection colour/icon inheritance | Real taxonomy (no port) |
| `browse.js` icons `GAMEPAD`/`COMPUTER`/`HANDHELD` | 59–66 | Two-tone 16×10 icons | `artwork.rs` `category_icon` |
| `browse.js` `cardImage` | 75–99 | Generic card face | `artwork.rs` `surface`/`draw_card_labels` |
| `browse.js` `backImage` | 205–225 | MagiK back per colour | `artwork.rs` `back_surface` |
| `browse.js` `slot` | 245–256 | Root fan (unchanged) | `launcher.rs` `slot_geometry` |
| `browse.js` `at`…`slotFor` | 257–278 | Row slots, hidden slots, cycling | New row table; `continuous_geometry` |
| `browse.js` `layoutCards` | 293–300 | Resting row | `build_carousel_plan` settled |
| `browse.js` `slideLeft` | 323–367 | Row step | `build_carousel_plan` slide branch |
| `browse.js` `trick` | 369–432 | Level trick with travel | `level_trick.rs` |
| `browse.js` `pageContext`/`openSysPage`/`key` | 556–585 | Enter the page from a card | `launcher.rs` input; `navigation_transition_for_intent` |
| `hub.js` `build` | 30–54 | Hub panel | New Slint hub panel |
| `gamelist.js` constants | 16–22 | Device/screen/region geometry | `arcade_card.rs` constants |
| `gamelist.js` `build`/`setup`/`fillList` | 102–198 | Page content | Merged Slint page |
| `gamelist.js` `applyMode`/`toggle`/`openSection` | 199–253 | Select toggle, sections | Nav page mode |
| `gamelist.js` `pushBands` | 228–239 | Band push | Segmented band renderer |
| `gamelist.js` `zoomFrame`/`run` | 254–302 | Card → page reveal | Device-card reveal renderer |
| `gamelist.js` `shotSrc` | 50–89 | Placeholder screenshots | **Do not port** |
| `arcade.js` hub | 37–82 | Arcade hub and sections | Nav + Slint |
| `arcade.js` `zoomFrame` | 140–176 | Arcade reveal (lands on hub) | `arcade_card.rs` |
| `crt.js` `drawBackdrop`/`scrim` | 150–181 | CRT screenshot backdrop | `crt_backdrop_controller.rs` |
| `crt.js` Arcade hub | 207–300 | CRT Arcade hub, toggle, reveal | CRT page, `arcade_card.rs` `render_crt` |
| `crt.js` `crtCard`/`crtRootCard`/`crtBack`/`drawBCard` | 610–686 | CRT faces, back, squeeze draw | CRT faces in `artwork.rs` |
| `crt.js` row constants | 636–643 | CRT row | CRT row table |
| `crt.js` `stepCards`/`browseStep` | 702–742 | CRT step | slide branch (CRT) |
| `crt.js` trick | 760–809 | CRT trick | `level_trick.rs` |
| `crt.js` page | 833–972 | CRT hub/list, toggle, reveal | CRT page, band renderer, CRT reveal |
| `crt.js` `drawSystem` | 988–1006 | CRT hub rows | CRT hub panel |
| `crt.js` `browseKey` | 1053–1085 | CRT input | `launcher.rs` input |

## Appendix B: timing tables

| Motion | Total | Key windows |
|---|---|---|
| Row step | 460 ms | all cards `inOut`; end card angle linear 180°→14° over 0–460 |
| Level trick | 920 ms | EDGE 460; hero `inOut` over 0–920; gather 0–460 (vanish 440–460); deal EDGE + 20·i, 360 ms; chrome out 0–260, swap 414, in 506–866 |
| HDMI card → page | 1000 ms | window 0–760; face out 60–260; device 80–840; level out 40–280; header in 260–460; hub 500 + 26·i, 280 ms; footer 640–860; screen 760–960 |
| CRT card → page | 900 ms | window 0–680; face out 30–230; snapshot out 100–360; header in 260–440; scrim 420–720; hub rows 500 + 22·i, 260 ms; footer 560–760 |
| HDMI hub ↔ list | ≈ 460 ms | out 0–180 (−40 px); in from 120 + 26·i, 280 ms (+32 px) |
| CRT hub ↔ list | 340 ms | out 0–150 (−24 px); swap 150; in 150 + 14·i, 190 ms (+24 px) |

## Appendix C: review states

Open `prototype/web/index.html` with a hash. A trailing `-3` or `-4` picks
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


## Implementation commits

All commits build on `ac337f86e` from `nigel/collection-card-browse`.

1. **Document the parity contract and reference** (this commit): preserve the
   existing root renderer and motion; fix timing and clipping ambiguities.
2. **Protect root rendering**: deterministic root-frame regression evidence
   in both directions and native layouts, before changing geometry.
3. **Place nested cards in the row**: layout selection, native HDMI/CRT
   anchors, scale/overlap/dimming, short-level cycling, bounded plan capacity.
4. **Time nested steps explicitly**: 460 ms elapsed clock, clipped front,
   middle interpolation, linear end-card turn, short-level duplicate roles;
   root input and motion retain their current behaviour.
5. **Make level changes travel continuously**: shared source/destination
   hero poses, 920 ms clock and 460 ms handover, gather/deal and preparation.
6. **Unify system page behaviour**: hub/list mode, Arcade hub, Select and
   filtered sections, consistent Back and direct/launch-return entries.
7. **Animate panel toggles**: persistent device/backdrop and bounded raster
   bands with explicit timings and curves.
8. **Reveal cards into system pages**: parameterise the existing Arcade
   renderer, correct native CRT artwork window, hub landing and reverse.
9. **Update collection icons**: isolated artwork-only change.

Tests and offline review frames accompany the owning implementation commit.
No separate final test-only commit repairs earlier unvalidated changes.
Physical captures and Analytics validate scanout, input and performance after
portable checks pass; host renders do not establish device performance.
An edge hold is a readiness fallback and must be reported separately from
920 ms nominal motion. Prefetch/readiness should prevent it in normal use.
Portrait and non-240p CRT retain supported production geometry until their
new row geometry is explicitly reviewed.
