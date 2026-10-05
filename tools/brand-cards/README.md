# Maker and family cards

Fifteen graphic identities cover the 21 maker/family cards in Consoles,
Handhelds and Computers. The six home artworks and individual system renders
are unchanged. Old family RGB files are removed from the runtime pack.

These designs use the original SVG logo contours, flat colour fields and no
baked floor/reflections. `logos/` preserves the source vectors; `sources.json`
records attribution, copyright status, URLs and source checksums. The app's
Card Artwork license page includes the same credits. Acorn uses plain company
name typography because the historical logo source is marked non-free.

Run from the repository root:

```sh
uv run --with pillow --with cairosvg python tools/brand-cards/render.py --install
scripts/cargo run --manifest-path apps/mister/Cargo.toml --features ui \
  --example maker_card_review -- outputs/brand-cards/production
```

The generator needs ImageMagick. It emits editable SVG compositions in `cards/`
and ignored 1440x2016 masters, 360x504 runtime pixels, a contact sheet and a
self-contained dark/colour comparison page in `outputs/brand-cards/`.
The contact sheet/page have illustrative counts; the production example uses
the actual native fonts, card frame, filtering and RGB565 renderer at HDMI,
portrait and CRT sizes. Neither is a physical device capture.

The installed pack uses the colour variants. Dark variants are design
alternatives only; no focus-triggered palette switching or second runtime
texture set is introduced. The existing renderer still controls lighting,
selection, native labels, counts and reflections. Light identities use a black
metadata area to preserve native text contrast.

`contains_name` in `index.json` suppresses the native card title only after a
valid wordmark image loads. Icon-only marks retain the native company name;
missing/invalid artwork always retains its generic title. Other manifest entries
default to false, preserving existing root and system behaviour.
