# P2 — half-res render + 2× nearest upscale

**Status:** deployed and bench-complete (2026-06-04). Binary **2.3 MB** (`release-device` + Press Start 2P embed).

## What changed

- Slint scenes: **960×540** logical layout, [`PressStart2P-Regular.ttf`](../../rust/ui/fonts/PressStart2P-Regular.ttf) (SIL OFL).
- Render into `cached` at 960×540; after vsync, **`copy_rows_scaled`** replicates each pixel 2×2 into `/dev/fb0`.
- HDMI / FPGA routing unchanged: **1920×1080**, `xoff=yoff=0`.

## Run automated bench (TSV + PNG)

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh P2 --replace-label --device
```

`bench-toolchain.sh` sets `MISTER_PIXEL_SCALE=2` on device (override with `MISTER_PIXEL_SCALE=1` only for debug — layouts expect scale 2).

Compare **`scene`** rows: label **P2** vs historical **A0** / **A3** in [`results.tsv`](results.tsv).

## TV spot-check (production-like)

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh --fast

MISTER_IP=192.168.1.117 MISTER_PASS=1 uv run python scripts/mister_ssh.py run \
  'kill -9 $(pidof MiSTer) 2>/dev/null; MISTER_PIXEL_SCALE=2 /media/fat/mister-magic/mister-magic-fb ui static_ui 0'
```

Then try `ui launcher 0`, `ui full_motion 20`.

## Results (P2 vs A3 production, same device)

All scenes `visual_ok=yes`; PNGs under `P2-*-fb.png` show 2×2 blocks + Press Start 2P.

| scene | A3 render_us | P2 render_us | A3 copy_us | P2 copy_us | A3 fps | P2 fps |
|-------|-------------|-------------|-----------|-----------|--------|--------|
| demo | 2581 | **676** | 8277 | 10375 | 59 | 60 |
| full_motion | 2547 | **666** | 8669 | 10339 | 59 | 60 |
| static_ui | 1 | 1 | 1 | 1 | 61 | 61 |
| local_motion | 367 | **79** | 1049 | 1577 | 60 | 60 |
| text_heavy | 55728 | **23412** | 14013 | 17298 | 12 | 20 |
| solid_fill | 19647 | **4879** | 13570 | 16577 | 24 | 30 |
| list_scroll | 34862 | **13903** | 12443 | 15605 | 18 | 29 |

**Takeaways:**

- **Render:** ~4× faster on motion scenes (quarter pixels); ~2× on `text_heavy` / `list_scroll` (glyph-heavy Press Start still costly).
- **Copy:** ~20–25% higher on full-width dirty (`demo`, `full_motion`) — upscale writes 4× fb pixels per logical row; logical `rows_avg` is ~half of A3 (310 vs 619) because dirty height is in 540p space.
- **FPS:** Locked **60** on `demo` / `full_motion` / `local_motion`; `text_heavy` improves 12→20 but not 60 (font/glyph bound).
- **CPU:** Similar peaks (~80% on `text_heavy`).

## Hypotheses (confirmed)

| Metric | vs A3 (1920 DejaVu) |
|--------|---------------------|
| `render_us` | Lower (strong on motion, moderate on text) |
| `copy_us` | Higher on wide dirty bands |
| `fps` | 60 on motion; text/list still sub-60 |
| PNG | 2×2 pixels, bitmap font |

## Revert

Restore DejaVu + 1920×1080 `.slint`, remove `ui_display.rs` / `copy_rows_scaled`, set `MISTER_PIXEL_SCALE` default to 1.
