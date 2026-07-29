# Particle technique visual targets

These ten images define attainable visual targets for the proposed MiSTer MagiK
particle techniques. They are concept frames, not captures of the current
renderer.

Every final PNG is exactly 960×540 with no alpha channel. Its decoded red and
blue channels contain only the 32 values representable by five bits, and its
green channel contains only the 64 values representable by six bits. The PNGs
therefore preview exact RGB565 colour precision while remaining convenient to
inspect in ordinary image tools.

The generated source images were resized to the runtime geometry and quantized
with:

```text
R5 = round(R8 * 31 / 255)
G6 = round(G8 * 63 / 255)
B5 = round(B8 * 31 / 255)
```

The stored eight-bit PNG channels are the corresponding expanded RGB565 values.
Validation found zero non-RGB565 colours in every final image.

The images deliberately retain 60–88% true-black pixels, use bounded sprite and
trail sizes, and avoid geometry, volumetrics, lighting, or fluid simulation that
would be unrealistic for MagiK's software RGB565 renderer.

See `manifest.json` for hashes and validation metadata and `prompts.md` for the
generation briefs.

Deterministic local renderer captures use the checked-in seed and advance the
showcase at 60 Hz so stateful techniques reproduce their device cadence:

```text
mister-magik-particle-preview --demo grid-flocking --time-ms 15000 \
  --hud off --output /tmp/grid-flocking.ppm
```

The command refuses to overwrite an existing output file.
