# Standalone RGB565 Decimator Benchmark

This pure-C ARMv7 benchmark compares the production scalar and NEON 2x RGB565
decimators with exploratory kernel shapes. It runs without Rust, Slint, the
MagiK agent protocol, or application startup, while linking the exact production
helper from `magik-gui/src/framebuffer/downsample_neon.c`.

From the repository root:

```bash
scripts/profile-rgb565-decimator.sh LABEL
```

Useful controls are `--samples N`, `--runs N`, `--cpu N`, and `--skip-build`.
The runner builds for Cortex-A9, copies the binary to the MiSTer's volatile
`/tmp`, pins it to the requested CPU, and writes raw and aggregate TSV evidence
under `build/rgb565-decimator-bench/`.

Each run checks exact output and checksums before timing contiguous 960x540,
padded 960x540, and odd 959x539 inputs. It also verifies the production NEON
entry point's deliberately misaligned-source fallback. Kernel order rotates per
sample to reduce order and cache bias. Generated evidence is intentionally
ignored by Git.
