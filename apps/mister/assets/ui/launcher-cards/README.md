# Production launcher artwork

The runtime pack contains 42 text-free 360x504, eight-bit sRGB `.rgb888` files:
six root cards, thirteen Nintendo/Sega console and maker cards, and twenty-three
computer and maker cards. `index.json` maps stable taxonomy IDs to filenames and
SHA-256 checksums. Missing systems retain generic faces. Rejected model trials
are excluded. Source models and textures remain in the private assets repository.
Model credits, licence links and modifications are in
`apps/mister/licenses/CARD-ARTWORK.txt` and the app's **Card artwork** license entry.

## Installation and loading

The ZIP and Downloader packages install the index and pixels under
`/media/fat/mister-magik/assets/ui/launcher-cards/`. Development builds resolve
`/media/fat/mister-magik-dev/assets/ui/launcher-cards/` through the existing
`DevicePaths` layout contract. `magik deploy` currently transfers only the binary;
this PR does not add development-service asset transfer. A Dev installation must
already contain the artwork directory. Host previews default to this source-tree
directory. `MISTER_MAGIK_CARD_ASSETS` overrides the directory for previews or an
explicitly configured installation.

The app reads the index and validates pixel sizes/checksums during initial card
preparation and on the existing background preparation worker. Each bounded
level cache keeps its sources with its prepared faces. Counts, labels and clock
refreshes reuse those sources; changes to ordered artwork IDs invalidate them.
No file reads or checksums occur in frame rendering. Restart after replacing an
installed pack; assets are not hot-reloaded. Absent, incompatible or corrupt
files retain their carousel slots and show generic cards. Release validation
rejects incomplete or corrupt packs before publication.

## One source size for HDMI and CRT

One 360x504 RGB888 source supports both output routes. The renderer already
prepares and caches display-specific faces, including CRT pixel aspect ratio,
orientation and safe content area. Separate pre-quantized CRT files would discard
colour precision before filtering. Text and frames remain native renderer output.
This is the existing renderer's source resolution, not a new device-performance
claim; SD-card preparation latency has not been measured on hardware here.

The card sources are flattened images. Expanding hardware into a system hub
background needs a separate, larger transparent render (and independent layers
for independently moving controllers or monitors). This pack does not implement
that transition or treat the small card image as full-screen background art.

## Clean-background regeneration

All 42 cards, including the six original launcher cards, are rendered without
studio floor/sweep meshes, visible world backgrounds or compositor bloom.
Hardware, cameras, materials and lights retain their scene settings. Arcade uses
the current front-on `MAGIK_01_ARCADE_CABINET` scene and its card camera, saved in
`tools/blender/card-studio/MiSTer-MagiK-Arcade-Cabinet.blend`; it does not use the
older angled Arcade scene in the original six-card studio. Transparent
1500x2100 masters are composited onto exact black and reduced in linear light to
360x504 RGB888. This changes only baked artwork; the launcher's mirrored
reflections remain enabled. Pure-black interior pixels still traverse the opaque
card rendering path, so this is not a measured frame-time optimisation.

Use `tools/blender/card-studio/render_clean_background.py` with Blender and
ImageMagick. Point `--blend-dir` at the directory containing the original,
console and computer studios (and their private linked model libraries):

```sh
blender --background --python tools/blender/card-studio/render_clean_background.py -- \
  --blend-dir /path/to/card-studio \
  --index apps/mister/assets/ui/launcher-cards/index.json \
  --output /path/to/ignored/clean-background-renders
```

The script never saves over the studios. It emits transparent masters, runtime
RGB888 files, an updated index and a render manifest. It also exports exact-black
source masks in `masks/*.black1`: 360x504 bits, row-major, LSB-first within each
byte (45 bytes per row; 22,680 bytes per mask). A set bit means that the source RGB
pixel is exactly zero. The render manifest records the source and mask hashes.
These are opaque-black masks, not object transparency; the large PNG masters
retain object alpha separately. Runtime text/borders and complete filtering
footprints must be accounted for before any renderer fast path can use them.
The masks remain with the export outputs until a renderer consumes them; this
PR does not install unused masks onto the SD card. Review the renders before
copying `runtime/*.rgb888` and `index.json` into this directory. Keep the masters
outside Git. The original source archive
`MiSTer-MagiK-Blender-Dual-Scale-Satin-2026-09-20.zip` remains external.

For a separate, already composed opaque source, resize in linear light and
encode eight-bit sRGB:

```sh
magick SOURCE.png -alpha remove -colorspace RGB -filter Lanczos \
  -resize 360x504 -unsharp 0x0.55+0.65+0.015 -colorspace sRGB \
  -depth 8 RGB:DESTINATION.rgb888
```

Landscape prepares one linear-light 180x252 reduction per card, shares it between
compact/detail labels, and retains RGB8 artwork in the mip pyramid. Filtering,
angle lighting and alpha composition precede destination-space RGB565 dithering
on moving and resting cards. Native portrait/CRT layouts area-filter to their
output geometry and keep native bitmap text and silhouette coverage.

Scanout stays RGB565. The superseded 180x252 RGB565 source copies and quality
switches have been removed. No animation poses or angle atlases are stored.
