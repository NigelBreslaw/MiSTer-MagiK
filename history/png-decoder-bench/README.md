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
