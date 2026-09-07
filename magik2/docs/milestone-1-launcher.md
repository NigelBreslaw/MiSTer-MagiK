# Mini-MagiK static launcher baseline

The first launcher milestone presents a static RGB565 scene through Mini-MagiK.
It uses the 960x540 concept coordinate system, a fixed library summary rail,
five text-only category cards, and short darkened reflections. The fixture
values are visual test data: 6,842 games, 18 collections, and 126 favourites.
The cards use Arcade (1,752 games), SNK NEOGEO (324), Consoles (842),
Handhelds (126), and Computers (86).

Mini renders the scene into a packed `width * height` buffer and copies rows into
the presenter's stride-backed slot. A launcher frame is considered ready only
after `post()` and `settle_pending()` confirm a latched presentation. Session
preview servicing continues on idle iterations, while identical launcher frames
are not posted again after accessibility callbacks request redraws.

The consumer smoke scenario enters the retained Slint probe explicitly, checks
its existing counter/details behavior, enters launcher mode, waits for the
presented readiness state, checks Arcade selection, and writes a PNG derived
from the native `fpga-latched-scanout-slots` capture. A Slint screenshot is not
used as launcher visual evidence.

Focused checks:

```text
scripts/cargo test --manifest-path crates/framebuffer-scenes/Cargo.toml --locked
scripts/cargo check --manifest-path magik2/probe/Cargo.toml --locked
uv run --project magik2/host pytest magik2/scenarios --collect-only
```

Device validation completed on 2026-09-07 using the typed Mini-MagiK workflow:

```text
scripts/magik2 deploy --app mini-magik
scripts/magik2 check --app mini-magik
```

Deployment reused the committed Mini artifact and started it at
`/media/fat/mister-magik2/mini-magik`. The smoke check passed on the remembered
MiSTer with a native `fpga-latched-scanout-slots` capture at 960x540 RGB565;
the retained PNG is `build/magik2-results/20260907T192939Z-40de9bc8a191/smoke.png`.
The check waited for custom-scene readiness after post/settle, then sampled
native metrics across 807ms. It observed unchanged presentation and physical
post counters (2 and 2), the same process identity (PID 4371) and artifact hash
(`0751282d03aa8a17d568f6277f6535814659570dd42cb770cae32ecc2e12b4d7`), and no
evidence error. The PNG is derived from that native capture, never from a
Slint window screenshot.

Joystick browsing, flips, hold behavior, real catalog data, and game launching
remain later milestones.

## Scaler destination correction

The initial capture validated only the 960x540 source pixels. Mini used a
source-sized destination rectangle, so the UI occupied one quarter of the
1920x1080 display. Source captures cannot establish final HDMI screen coverage.

Mini now queries Main's active `DisplayV1` state through the existing serialized
command transport and opens `HiddenLatchPresenter::open_for_plan`. The shared
display plan supplies both the source dimensions and the full destination scan
rectangle, including the existing pixel-repetition and CRT route rules. It does
not upscale the CPU-rendered buffer or change the installed video mode. Raw
`UIO_GET_VRES` timing is not used as a substitute: this device reports core
timing of 529x240 while Main's active mode is HDMI 1920x1080. Unresolved Main
modes fail explicitly rather than silently choosing a source-sized rectangle.

Device smoke on 2026-09-07 recorded `source=960x540`, `scan=1920x1080`, and
`destination=1920x1080` in
`build/magik2-results/20260907T194040Z-92b554ca28d3/events.jsonl`.
The smoke now asserts source/capture agreement and destination/scan agreement.
Three display-contract tests, the existing source-stride/scaled-destination
regression, three consumer geometry tests, and Mini device smoke passed.
These checks verify the programmed geometry and acknowledged presentation;
the native capture remains pre-scaler evidence, not an HDMI capture.
