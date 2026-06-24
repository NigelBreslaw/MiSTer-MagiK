# AI-Visible Preview Benchmark Failures - 2026-06-24

Scope: preview benchmark scripts.

Parent baseline: `485d8d6`.

## Cause

The preview benchmark scripts could produce misleading output for agents:

- Required-motion traces printed `moving_frames=0` without a machine-readable
  validity row.
- Trace, log, status, CPU SVG, and screenshot paths were only human text.
- Visual captures were copied without dimensions or nonblank validation.
- Gate failures did not emit a structured reason before returning nonzero.

## Fix

- Emit `run_context_tsv`, `artifact_tsv`, `validity_tsv`, and
  `motion_valid_tsv` rows from preview-scroll runs.
- Save and report a `scripts/mister status` artifact for each preview run.
- Validate captured PNG dimensions and nonblank pixel variation.
- Emit structured validity rows from the final preview gate parser.

## Hardware Evidence

- Command: `scripts/profile-preview-scroll.sh 15 held-scroll ITEM02-AI-VISIBLE-SMOKE --skip-build --visual-captures 1`
- Artifacts:
  - `build/preview-scroll-profiles/ITEM02-AI-VISIBLE-SMOKE-arcade.tsv`
  - `build/preview-scroll-profiles/ITEM02-AI-VISIBLE-SMOKE-arcade.log`
  - `build/preview-scroll-profiles/ITEM02-AI-VISIBLE-SMOKE-arcade.status.txt`
  - `build/preview-scroll-profiles/ITEM02-AI-VISIBLE-SMOKE-visuals/idx000.png`
- Visual artifact row: `width=960`, `height=540`, `nonblank=true`.
- Motion validity row: `valid=1`, `moving_frames=876`,
  `fractional_visual_index_frames=767`, `visual_max=109.500`.
- Final validity row: `valid=1`, `invalid_reason=ok`.

## Validation

- `scripts/profile-preview-scroll.sh --self-test`
- `scripts/gate-preview-60fps.sh --self-test`
- `scripts/test-host-tools.sh`
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
- `cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings`
