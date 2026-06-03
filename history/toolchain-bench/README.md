# Toolchain benchmark log

Compare cross-compile / release-profile options for `mister-magic-fb` on the MiSTer.
Each run exercises **seven** Slint bench scenes (see [`rust/ui/bench/README.md`](../../rust/ui/bench/README.md)).

## Run

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh A0 --clean
```

| Flag | Meaning |
|------|---------|
| `A0`, `A1`, … | Toolchain label (TSV + PNG prefix) |
| `--clean` | `cargo clean` before build |
| `--skip-build` | Reuse release binary |
| `--skip-device` | Host metrics only (placeholder rows per scene) |
| `--replace-label` | Drop existing TSV rows for this label before appending (re-run A0) |
| `--scene-secs N` | Seconds per scene (default **15**, ~105s device for 7 scenes) |

## Visual artifacts (gitignored)

For label `A0` and scene `static_ui`:

- `A0-static_ui-fb.png` — framebuffer capture after that scene
- `A0-static_ui-ui.log` — stdout from `mister-magic-fb ui static_ui N`

## TSV columns

One row **per scene**. Compare the same `scene` across toolchain labels (A0 vs A1).

`static_ui` and `local_motion` usually show much lower `copy_us` / `rows_avg` than `full_motion`.

## Experiment matrix

| Label | Cargo.toml | `.cargo/config.toml` |
|-------|------------|----------------------|
| A0 | Baseline | No ARM rustflags |
| A1 | Same | `cortex-a9`, `+neon,+vfp3` |
| A2 | `lto="fat"`, `codegen-units=1` | No rustflags |
| A3 | A2 | A1 rustflags |

Run **A0** before editing toolchain files.
