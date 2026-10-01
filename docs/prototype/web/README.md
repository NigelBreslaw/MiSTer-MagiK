# Web prototype (card launcher, system pages, CRT)

The HTML/JS prototype that production UI work in
[`../../ui-prototype-parity.md`](../../ui-prototype-parity.md) is measured
against. It is design reference, not shipped code: numbers, timings and
behaviour here are the spec; the Rust/Slint implementation is the product.

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
listed in the parity plan.
