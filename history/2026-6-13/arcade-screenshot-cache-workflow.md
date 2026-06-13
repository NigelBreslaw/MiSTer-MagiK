# Arcade screenshot cache workflow - 2026-06-13

This note records the screenshot cache layout and intended update workflow so a
future agent can refresh arcade previews without re-reading the preview loader
code.

## Cache layers

There are three distinct layers:

- **Original screenshots on the MiSTer:** `/media/fat/_Arcade/media/screenshot`
- **Inspectable resized PNGs on the Mac:** `png-hybrid-320x320/*.png`
- **Runtime raw previews on the MiSTer:**
  `/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320/*.rgb565`

The resized PNGs are for human inspection only. Do not copy them to the MiSTer
unless deliberately debugging a PNG path. The launcher should load the prebuilt
`.rgb565` files.

Legacy cache directories seen on the MiSTer:

- `/media/fat/_Arcade/media/screenshot-magik/png-nearest-320x320`
- `/media/fat/_Arcade/media/screenshot-magik/raw565-nearest-320x320`
- `/media/fat/_Arcade/media/screenshot-magik/png-box-320x320`
- `/media/fat/_Arcade/media/screenshot-magik/png-lanczos-320x320`

The old active paired cache was 904 resized PNGs in `png-nearest-320x320` and
904 raw files in `raw565-nearest-320x320`.

After the 2026-06-13 refresh, the MiSTer cache directory was cleaned so only
`raw565-hybrid-320x320` remained under `screenshot-magik`.

## Safe cleanup rule

Never delete `/media/fat/_Arcade/media/screenshot`. Those are the originals and
the source of truth.

Only delete or recreate directories under
`/media/fat/_Arcade/media/screenshot-magik`.

## Source set

Use all real original screenshots from the MiSTer. Skip AppleDouble metadata
files whose filenames start with `._`.

Source extensions to include:

- `.png`
- `.jpg`
- `.jpeg`

Counts discovered on 2026-06-13:

- 904 real PNG originals
- 6 real JPG originals
- 827 AppleDouble metadata files to ignore

Use the source file stem for cache filenames. For example:

- `astrass.jpg` -> `astrass.png` locally for inspection
- `astrass.jpg` -> `astrass.rgb565` for the runtime cache

## Hybrid resize policy

Fit each image inside `320x320` while preserving aspect ratio.

- If fitting requires downscaling, resize with Lanczos.
- If fitting requires upscaling, resize with nearest neighbour.
- If the image already lands on the target dimensions, leave it unchanged.

Examples from the existing aspect-fit rule:

- `224x256` -> `280x320`
- `384x224` -> `320x187`
- `320x240` -> unchanged
- `224x384` -> `187x320`

Generate both local inspectable PNGs and runtime `.rgb565` files from the same
resized RGB image data.

## Raw565 file format

The runtime `.rgb565` preview format is:

- Magic header: `MM56501\0`
- Little-endian `u32` width
- Little-endian `u32` height
- Little-endian `u32` stride bytes
- Little-endian RGB565 pixel rows
- Row stride aligned to 16 bytes
- Zero padding after each row as needed

RGB8 to RGB565 packing:

```text
((r & 0xf8) << 8) | ((g & 0xfc) << 3) | (b >> 3)
```

Store each `u16` word little-endian.

## Update workflow

1. Pull or otherwise stage the real originals from the MiSTer screenshot
   directory.
2. Generate local inspectable PNGs under `png-hybrid-320x320`.
3. Generate matching local `.rgb565` files under `raw565-hybrid-320x320`.
4. Inspect representative PNGs: vertical, horizontal, small/upscaled, and
   large/downscaled.
5. Transfer only `.rgb565` files to
   `/media/fat/_Arcade/media/screenshot-magik/raw565-hybrid-320x320`.
6. Point MiSTer MagiK at the hybrid raw565 cache.
7. After verification, remove stale cache directories under
   `/media/fat/_Arcade/media/screenshot-magik` if desired.

## Acceptance checks

- Local generated `.rgb565` count matches the real source image count.
- MiSTer deployed `.rgb565` count matches the local generated count.
- Arcade UI loads previews with `MISTER_PREVIEW_FORMAT=raw-rgb565`.
- `/media/fat/_Arcade/media/screenshot` remains unchanged.

## Rust builder

The repeatable host-side builder is part of the Rust `tools/mister` project. Run
it in release mode for the real conversion:

```bash
cargo build --release --manifest-path tools/mister/Cargo.toml

tools/mister/target/release/mister preview-cache-build \
  --input build/arcade-screenshot-cache/hybrid-20260613/source/screenshot \
  --output build/arcade-screenshot-cache/hybrid-20260613 \
  --max 320
```

The converter uses the standard Rust `image` crate for PNG/JPEG decode, resize,
and inspectable PNG output. It uses `rayon` to process images across all CPU
cores. On 2026-06-13, the release binary converted the 910-file source set in
`425 ms` of tool-reported conversion time (`0.66 s` wall clock with per-file
output redirected to a log) on the Mac.

The runtime default preview resize filter is `hybrid`, so the default raw565
cache path is `screenshot-magik/raw565-hybrid-320x320/*.rgb565`.

For deployment, package the raw565 directory as a clean tarball with macOS
copyfile metadata disabled:

```bash
COPYFILE_DISABLE=1 tar -cf \
  build/arcade-screenshot-cache/hybrid-20260613/raw565-hybrid-320x320.clean.tar \
  -C build/arcade-screenshot-cache/hybrid-20260613 \
  raw565-hybrid-320x320
```

Do not upload large cache archives to MiSTer `/tmp`; it is a small tmpfs and
will fill. Upload the tarball under `screenshot-magik`, extract it there, then
delete the tarball. If GNU tar prints `LIBARCHIVE.xattr.com.apple.provenance`
warnings, use `--warning=no-unknown-keyword` during extraction.
