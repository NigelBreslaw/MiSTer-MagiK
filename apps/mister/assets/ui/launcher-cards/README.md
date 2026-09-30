# Production launcher artwork

The six text-free `.rgb888` files contain 360x504 RGB from the accepted 5:7
Blender renders. Runtime order is encoded in their filenames. The source archive
`MiSTer-MagiK-Blender-Dual-Scale-Satin-2026-09-20.zip` remains external;
`01_arcade` comes from the accepted Arcade prototype render.

Generate the source assets in linear light, then encode eight-bit sRGB:

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
