# Arcade presentation assets

`cabinet-483x519.rgb565` is the front-on HDMI Arcade cabinet used at its
native 483x519 size. Its 320x320 screen opening begins at `(82, 61)` within
the asset, so game screenshots remain on their native pixel grid.

The accepted source is `arcade-cabinet.png` from the Arcade UI prototype. The
Blender render's near-black world pixels are clamped to true black before the
image is flattened and packed as little-endian RGB565 without resizing. This
prevents its matte from showing against the launcher's `#000000` background:

```sh
magick arcade-cabinet.png -channel RGB -black-threshold 4% +channel \
  -background black -alpha remove PNG24:/tmp/cabinet.png
ffmpeg -hide_banner -loglevel error -i /tmp/cabinet.png \
  -f rawvideo -pix_fmt rgb565le -y cabinet-483x519.rgb565
```

The source PNG remains an external design source and is not committed here.
