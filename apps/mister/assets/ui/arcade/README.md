# Arcade presentation artwork

`cabinet-483x519.rgb888` contains the accepted front-on cabinet render at its
native HDMI destination size. The screen opening is composed with the live
320x320 game image; screenshots and UI text retain their native pixel grid.

The accepted PNG is clamped to black and flattened before RGB encoding:

```sh
magick arcade-cabinet.png -channel RGB -black-threshold 4% +channel \
  -background black -alpha remove -depth 8 RGB:cabinet-483x519.rgb888
```

The source PNG remains external. Runtime prepares premultiplied mip levels off
the UI thread and shares the immutable artwork between rendering workers. The
live two-worker reveal filters before destination-space RGB565 quantisation;
the resting backdrop uses the same source and dither phase. The superseded
RGB565 asset and nearest-sampled HDMI reveal have been removed. Output remains
RGB565 and there is no complete-frame animation cache.
