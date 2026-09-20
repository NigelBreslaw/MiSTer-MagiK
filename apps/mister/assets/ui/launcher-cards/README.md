# Production launcher cards

These files are text-free 180x252 little-endian RGB565 textures for the 5:7
card launcher. Runtime order is encoded in the filenames and is independent of
the older source-render numbering.

The accepted source renders were supplied in
`MiSTer-MagiK-Blender-Dual-Scale-Satin-2026-09-20.zip`. The archive remains an
external design source and is not committed here.

Do not convert a smooth near-black render and expect quantisation or runtime
dithering to repair it: RGB565 exposes broad dark gradients as bands. Preserve
fine matte texture, broad lighting planes and local contrast, especially on the
Favourites heart.

Downsample once in linear RGB, sharpen at target size, return to sRGB, and pack
without further colour effects:

```sh
magick SOURCE.png -alpha remove -colorspace RGB -filter Lanczos \
  -resize 180x252 -unsharp 0x0.55+0.65+0.015 -colorspace sRGB \
  PNG24:/tmp/card.png
ffmpeg -hide_banner -loglevel error -i /tmp/card.png \
  -f rawvideo -pix_fmt rgb565le -y DESTINATION.rgb565
```

Decode the packed result back to PNG and inspect it at 180x252 before
committing. A clean high-resolution source does not prove that its RGB565 form
is free of banding.
