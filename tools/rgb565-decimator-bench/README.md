# Standalone RGB565 Decimator Benchmark

This pure-C ARMv7 benchmark measures the production scalar 2x RGB565 decimator.
It runs without Rust, Slint, the MagiK agent protocol, or application startup,
while linking the exact production helper from
`magik-gui/src/framebuffer/downsample_scalar.c`.

From the repository root:

```bash
scripts/profile-rgb565-decimator.sh LABEL
```

Useful controls are `--samples N`, `--runs N`, `--cpu N`, and `--skip-build`.
The runner builds for Cortex-A9, copies the binary to the MiSTer's volatile
`/tmp`, pins it to the requested CPU, and writes raw and aggregate TSV evidence
under `build/rgb565-decimator-bench/`.

Each run compares output against a simple indexed reference before timing
contiguous 960x540, padded 960x540, and odd 959x539 inputs. Generated evidence
is intentionally ignored by Git.
