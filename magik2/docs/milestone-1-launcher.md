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
the retained PNG is `build/magik2-results/20260907T192514Z-0efe147b88c6/smoke.png`.
The check waited for custom-scene readiness after post/settle, then sampled
native metrics across at least 600ms. It observed unchanged presentation and
physical post counters, the same process identity and artifact hash, and no
evidence error. The PNG is derived from that native capture, never from a Slint
window screenshot.

Joystick browsing, flips, hold behavior, real catalog data, and game launching
remain later milestones.
