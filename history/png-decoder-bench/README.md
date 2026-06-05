# PNG decoder benchmark

Deterministic MiSTer screenshot decode benchmark.

Fixtures live in `rust/benches/png-fixtures/` and are copied to the device at
`/media/fat/mister-magic/png-fixtures/`. They were selected from real MiSTer
arcade screenshots to span small/simple through larger/busier PNGs.

Command:

```bash
MISTER_PREVIEW_FIXTURE_REPEATS=20 /media/fat/mister-magic/mister-magic-fb preview-bench fixtures
```

## Fixtures

| File | Encoded bytes | Dimensions | PNG type |
|---|---:|---:|---|
| `01-sbrkoutct.png` | 397 | 224x256 | 8-bit indexed |
| `02-breakout.png` | 838 | 230x688 | 8-bit indexed |
| `03-btime.png` | 2,783 | 240x240 | 8-bit RGB |
| `04-tapperg.png` | 6,303 | 512x480 | 8-bit indexed |
| `05-1943kai.png` | 6,545 | 224x256 | 8-bit indexed |
| `06-akumajoun.png` | 11,779 | 256x224 | 8-bit RGB |
| `07-mmatrixj.png` | 12,095 | 384x224 | 8-bit indexed |
| `08-gigawing.png` | 23,367 | 384x224 | 8-bit indexed |
| `09-rtypeleo.png` | 23,655 | 320x240 | 8-bit indexed |
| `10-vsav.png` | 25,078 | 384x224 | 8-bit indexed |

## Results

### BASE-PNG017-20260605

Decoder: direct `png 0.17.16`.

| File | Read avg ms | Decode avg ms | Total avg ms | Decode p90 ms | Total p90 ms |
|---|---:|---:|---:|---:|---:|
| `01-sbrkoutct.png` | 0.151 | 3.643 | 3.805 | 3.663 | 3.821 |
| `02-breakout.png` | 0.174 | 10.729 | 10.913 | 10.781 | 10.959 |
| `03-btime.png` | 0.180 | 5.059 | 5.250 | 5.121 | 5.341 |
| `04-tapperg.png` | 0.189 | 16.727 | 16.926 | 16.809 | 17.016 |
| `05-1943kai.png` | 0.173 | 3.864 | 4.049 | 3.844 | 4.023 |
| `06-akumajoun.png` | 0.487 | 5.401 | 5.903 | 6.596 | 6.839 |
| `07-mmatrixj.png` | 0.208 | 5.766 | 5.984 | 5.888 | 6.099 |
| `08-gigawing.png` | 0.233 | 8.008 | 8.253 | 8.062 | 8.313 |
| `09-rtypeleo.png` | 0.243 | 7.565 | 7.820 | 7.589 | 7.827 |
| `10-vsav.png` | 0.254 | 8.006 | 8.271 | 8.053 | 8.290 |

Aggregate over 200 samples:

| Metric | Value |
|---|---:|
| Read avg | 0.229 ms |
| Read p90 | 0.267 ms |
| Decode avg | 7.477 ms |
| Decode p90 | 10.913 ms |
| Decode max | 16.888 ms |
| Total avg | 7.717 ms |
| Total p90 | 12.492 ms |
| Total max | 17.075 ms |

### PNG018-20260605

Decoder: direct `png 0.18.1`.

| File | Read avg ms | Decode avg ms | Total avg ms | Decode p90 ms | Total p90 ms |
|---|---:|---:|---:|---:|---:|
| `01-sbrkoutct.png` | 0.144 | 3.434 | 3.588 | 3.447 | 3.614 |
| `02-breakout.png` | 0.169 | 9.267 | 9.446 | 9.302 | 9.479 |
| `03-btime.png` | 0.178 | 3.328 | 3.516 | 3.364 | 3.611 |
| `04-tapperg.png` | 0.184 | 14.486 | 14.680 | 14.567 | 14.749 |
| `05-1943kai.png` | 0.179 | 3.699 | 3.887 | 3.711 | 3.882 |
| `06-akumajoun.png` | 0.201 | 4.317 | 4.529 | 4.334 | 4.534 |
| `07-mmatrixj.png` | 0.210 | 5.564 | 5.785 | 5.702 | 5.919 |
| `08-gigawing.png` | 0.246 | 6.667 | 6.923 | 6.686 | 6.934 |
| `09-rtypeleo.png` | 0.241 | 6.289 | 6.541 | 6.331 | 6.566 |
| `10-vsav.png` | 0.246 | 6.954 | 7.216 | 7.075 | 7.350 |

Aggregate over 200 samples:

| Metric | Value |
|---|---:|
| Read avg | 0.200 ms |
| Read p90 | 0.252 ms |
| Decode avg | 6.400 ms |
| Decode p90 | 9.796 ms |
| Decode max | 14.838 ms |
| Total avg | 6.611 ms |
| Total p90 | 10.012 ms |
| Total max | 15.088 ms |

Compared with `png 0.17.16`, `png 0.18.1` is about 14.4% faster on average
decode time for this fixture set.

### ZUNEPNG-20260605

Decoder: direct `zune-png 0.5.2`, using `png_set_strip_to_8bit(true)` and
`png_set_add_alpha_channel(true)`.

| File | Read avg ms | Decode avg ms | Total avg ms | Decode p90 ms | Total p90 ms |
|---|---:|---:|---:|---:|---:|
| `01-sbrkoutct.png` | 0.110 | 1.064 | 1.184 | 1.181 | 1.445 |
| `02-breakout.png` | 0.440 | 3.640 | 4.091 | 4.131 | 4.815 |
| `03-btime.png` | 0.130 | 2.033 | 2.173 | 2.040 | 2.183 |
| `04-tapperg.png` | 0.204 | 5.527 | 5.746 | 5.575 | 5.770 |
| `05-1943kai.png` | 0.140 | 1.571 | 1.720 | 1.602 | 1.753 |
| `06-akumajoun.png` | 0.167 | 2.335 | 2.511 | 2.363 | 2.539 |
| `07-mmatrixj.png` | 0.195 | 2.477 | 2.682 | 2.633 | 2.838 |
| `08-gigawing.png` | 0.223 | 3.158 | 3.391 | 3.197 | 3.441 |
| `09-rtypeleo.png` | 0.208 | 3.045 | 3.263 | 3.070 | 3.287 |
| `10-vsav.png` | 0.231 | 3.337 | 3.579 | 3.375 | 3.647 |

Aggregate over 200 samples:

| Metric | Value |
|---|---:|
| Read avg | 0.205 ms |
| Read p90 | 0.248 ms |
| Decode avg | 2.819 ms |
| Decode p90 | 4.655 ms |
| Decode max | 5.879 ms |
| Total avg | 3.034 ms |
| Total p90 | 5.659 ms |
| Total max | 9.173 ms |

Compared with `png 0.18.1`, `zune-png 0.5.2` is about 56.0% faster on average
decode time for this fixture set.

### ZUNEPNG-RGB8-20260605

Decoder/display path: direct `zune-png 0.5.2` without adding alpha, then
`Image::from_rgb8()` / `Rgb8Pixel`.

| File | Read avg ms | Decode avg ms | Total avg ms | Decode p90 ms | Total p90 ms | Decoded bytes |
|---|---:|---:|---:|---:|---:|---:|
| `01-sbrkoutct.png` | 0.136 | 1.274 | 1.417 | 1.334 | 1.431 | 172,032 |
| `02-breakout.png` | 0.208 | 4.213 | 4.432 | 4.220 | 4.385 | 474,720 |
| `03-btime.png` | 0.191 | 1.005 | 1.203 | 1.045 | 1.194 | 172,800 |
| `04-tapperg.png` | 0.251 | 6.862 | 7.125 | 7.051 | 7.272 | 737,280 |
| `05-1943kai.png` | 0.187 | 1.830 | 2.025 | 1.874 | 2.020 | 172,032 |
| `06-akumajoun.png` | 0.244 | 1.602 | 1.855 | 1.641 | 1.962 | 172,032 |
| `07-mmatrixj.png` | 0.242 | 2.826 | 3.077 | 2.957 | 3.147 | 258,048 |
| `08-gigawing.png` | 0.292 | 3.515 | 3.815 | 3.540 | 3.736 | 258,048 |
| `09-rtypeleo.png` | 0.283 | 3.353 | 3.645 | 3.382 | 3.565 | 230,400 |
| `10-vsav.png` | 0.296 | 3.695 | 4.000 | 3.730 | 3.932 | 258,048 |

Aggregate over 200 samples:

| Metric | Value |
|---|---:|
| Read avg | 0.233 ms |
| Read p90 | 0.220 ms |
| Decode avg | 3.017 ms |
| Decode p90 | 4.564 ms |
| Decode max | 7.381 ms |
| Total avg | 3.259 ms |
| Total p90 | 6.120 ms |
| Total max | 8.847 ms |
| Decoded bytes avg | 290,544 |
| Decoded bytes max | 737,280 |

Compared with the `zune-png` RGBA8 path, RGB8 saves exactly 25% decoded image
storage, but decode time is about 7.2% slower on this fixture set. This may
still help the UI cache footprint, but it is not a PNG decode speed win.
