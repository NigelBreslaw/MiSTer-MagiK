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
`DevicePaths` layout contract. `magik deploy` now validates and installs the complete indexed pack through
the native Dev artwork installer before starting the app. Unchanged packs are
not uploaded again; an artwork change restarts an otherwise-current launcher
so cached generic faces are replaced.
The six root renders are also built into the binary as a fallback, so a Dev
installation without the artwork directory retains the canonical root workload.
Installed root artwork takes precedence when readable. Native 960x540 landscape additionally loads the indexed `.cardtex` entries; other geometries keep using RGB888 sources. Nested artwork still
requires the filesystem pack. The fallback costs about 3.3 MB in the binary and
is borrowed directly; it does not allocate six extra source buffers.

Host previews default to this source-tree directory. `MISTER_MAGIK_CARD_ASSETS`
overrides it for previews or an explicitly configured installation. Preview
keys come from `CardLevelSnapshot`, independently of displayed labels.

Packaging/Downloader verify SHA-256 checksums. Runtime loading checks the index
format and exact pixel length, with no repeated content hashing. The shared
bounded-file reader rejects non-regular files before opening and uses nonblocking
open plus a descriptor type check on Unix to close the FIFO/symlink race.

The face cache requests source pixels only for a cache miss, and releases each
source immediately after its compact/detail faces have been prepared. No level
cache retains full-size source images. Clock/selection refreshes reuse all faces;
changed card labels/counts reload only the affected source on the preparation
worker. Ordered artwork-key changes advance `asset_generation`, letting the
renderer invalidate its own cache.

Missing packs, invalid indexes/files and built-in root fallbacks are stable for
unchanged artwork keys. Declared nested sources with transient read failures
(including missing files) are retried on the worker with a 1–30 second backoff.
Count/label refreshes and prefetch do not bypass that retry schedule. Requests
and adoption wait for motion to settle; unsuccessful retries reuse fallback
faces and do not invalidate the visible scene. Successful files are not polled.

The 53 prepared entries add 8,120,232 bytes to the artwork distribution. They
contain label-free RGB8, RGB565 face surfaces and premultiplied RGBA horizontal
mips, bound to their renderer format version, category and colour. Current
names/counts are applied on the worker; only changed mip rows are rebuilt.
No prepared file contains private fonts or freezes game counts. Unsupported,
missing or malformed prepared data falls back to the original RGB888 source.
Other display geometries retain their existing native preparation path.

Generate or verify deterministic prepared files on the host:

```sh
scripts/cargo run --manifest-path crates/framebuffer-scenes/Cargo.toml --release \
  --example prepare_card_artwork -- apps/mister/assets/ui/launcher-cards
# Append --check to verify the committed files and index without writing.
```

The Dev installer advertises `card-artwork-v2`, verifies all declared file
checksums at installation and keeps its existing transactional replacement and
reconciliation. Packaging/Downloader also verify the prepared entries.
Runtime checks version/style, block lengths and exact declared file length,
without hashing contents again. A changed card reads its prepared entry on the
worker; raw-only fallback still costs 544,320 bytes.
A same-length file corrupted after installation may display incorrect pixels;
runtime intentionally does not hash artwork again.

No file reads occur in frame rendering. Successful assets are not watched for
replacement; restart after updating a pack. Release validation continues to
reject incomplete or checksum-invalid packs before publication.

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

## Company and family artwork

`brand-*.rgb888` replaces the former controller piles and 3D family text with
flat logo-led graphics. The 15 identities are shared by 21 maker/family entries;
the six root images and individual system images are unchanged. Source vectors,
credits and regeneration instructions are in `tools/brand-cards/`.

`contains_name` is optional and defaults to false. A valid image with a company
wordmark suppresses the duplicate native title; icon-only logos keep the name,
and all failed loads restore the generic title. Game counts remain native.
The installed set uses corporate colour fields; dark alternatives are review
outputs only, without adding another runtime image cache or focus policy.
