# Settings backdrop

`cog-backdrop-412x374.rgb565` is the Settings screen's cog: 412×374
little-endian RGB565, presented 1:1 at an integer position on HDMI landscape
and portrait. It is never scaled at rest, so no pixels are resampled on the
device. Rust converts it once to a Slint RGB8 image by bit replication; the
RGB565 software renderer then writes back exactly these pixels.

The source is the `05_SETTINGS | BACKDROP 2x` camera in
`MiSTer-MagiK-Card-Artwork-5x7.blend`: the card camera's position and angle
with twice the orthographic frame, so the centre 408×571.5 region of the
816×1142 render is exactly the launcher card's framing. The studio floor is
excluded from rendering.

Regenerate from `outputs/card-studio` (ignored design workspace):

```sh
Blender -b MiSTer-MagiK-Card-Artwork-5x7.blend --python render_settings_backdrop.py -- \
  renders/settings-backdrop/05_SETTINGS_BACKDROP_2x_816.png 816 192
Blender -b --python crop_settings_backdrop.py -- \
  renders/settings-backdrop/05_SETTINGS_BACKDROP_2x_816.png \
  renders/settings-backdrop/05_SETTINGS_BACKDROP_816_crop.png
Blender -b --python pack_settings_backdrop_rgb565.py -- \
  renders/settings-backdrop/05_SETTINGS_BACKDROP_816_crop.png \
  cog-backdrop-412x374.rgb565
```

The crop fades the cog's surroundings to true black and removes every pixel
that would quantise to black, then packs with a fixed 4×4 ordered dither
applied once at the displayed size. Its report gives the crop origin
(312, 327) in the 816×1142 render, which places the cog relative to the card
for the Settings transition.
