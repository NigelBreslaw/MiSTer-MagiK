# Settings cog source

`cog-backdrop-412x374.rgb888` contains one 412×374 sRGB source, without RGB565
quantisation or baked dithering. Production prepares shared RGBA mip levels off
the UI thread alongside the initial cards. The cog fades from 100% to 50% between 380 and 700 ms, matching the latest UI
guide. Its resting image is quantised after the same 50% fade. The expanding cog
uses the same
separable filtered sampler and ARM kernels as the Arcade cabinet. It filters
colour/coverage first and dithers at the final RGB565 destination coordinates.
The resting Slint backdrop uses the same source, quantised at its HDMI position
(-18, 97); bit replication preserves those RGB565 pixels exactly.

The source is the `05_SETTINGS | BACKDROP 2x` camera in
`outputs/card-studio/MiSTer-MagiK-Card-Artwork-5x7.blend`. Its transform is copied
from the evaluated `05_SETTINGS | CONCEPT 5x7` camera after activating the Settings
scene and updating its view layer. The backdrop uses twice the orthographic
frame and half the camera shifts, bypasses the card compositor overlays and
excludes the studio floor. Render at 816×1142 with 192 Cycles samples. No changes
are saved to the Blender scene.

The existing `crop_settings_backdrop.py` removes the near-black world and fades
the surroundings to true black. The verified crop is (312, 327, 412, 374), with
centre (205, 193). Export the cropped PNG's stored sRGB channels (Blender image
colour space `Non-Color`) as rounded 0–255, top-down RGB bytes. Do not apply the
old RGB565 packing script: final dithering belongs to the renderer.

The single source is 462,264 bytes and replaces the old 308,176-byte packed copy.
No transition frames or scale variants are stored. Mini's `settings-transition`
workload exercises the production sampler and forward/reverse timeline.
