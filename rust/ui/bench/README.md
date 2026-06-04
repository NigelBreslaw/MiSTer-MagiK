# Slint visual bench scenes

Each `.slint` file is a **960×540** logical scene (P2: **2× nearest upscale** to 1920×1080
HDMI, Press Start 2P). Run on-device (menu SIGSTOPped):

```bash
MISTER_PIXEL_SCALE=2 /media/fat/mister-magic/mister-magic-fb ui <scene> <seconds>
```

Legacy full-res benches used 1920×1080 DejaVu (A0 TSV rows).

```bash
/media/fat/mister-magic/mister-magic-fb ui <scene> <seconds>
/media/fat/mister-magic/mister-magic-fb scenes   # list names
```

| Scene | File | What it stresses |
|-------|------|------------------|
| `demo` | [`../app.slint`](../app.slint) | Original demo (full-width bar + orbit) |
| `full_motion` | `full_motion.slint` | Same motion workload, `bench:` label |
| `static_ui` | `static_ui.slint` | No `animation-tick` — tiny dirty region after frame 1 |
| `local_motion` | `local_motion.slint` | Small orb only — localized dirty rows |
| `text_heavy` | `text_heavy.slint` | 28 text lines + scroll — glyph/layout cost |
| `solid_fill` | `solid_fill.slint` | Large quadrant fills — rectangle rasterization |
| `list_scroll` | `list_scroll.slint` | `std-widgets` **ScrollView** + `for` rows, auto scroll (ListView ignores driven `viewport-y` on-device) |

Edit `.slint` files with the **Slint LSP** enabled in the IDE (Cursor `ReadLints` /
Problems panel) and fix all diagnostics before building.

`scripts/bench-toolchain.sh` runs **all** scenes, captures
`history/toolchain-bench/<label>-<scene>-fb.png`, and appends one TSV row per scene.

**P2 pixel-scale experiment:** [`history/toolchain-bench/P2-pixel-scale.md`](../../history/toolchain-bench/P2-pixel-scale.md)

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh P2 --replace-label --device
```
