# Held-Scroll Benchmark Truth - 2026-06-24

Scope: production Arcade preview benchmark harness.

Parent baseline: `9c9ee441`.

## Cause

Summary-first launcher startup can set `catalog_ready=true` before the full
Arcade game rows are hydrated. The held-scroll benchmark attempted to step in
that state, `launcher_bench_step` returned `false`, but the benchmark loop still
advanced its synthetic step index. When rows arrived later, held-scroll passed
`previous_dir=1` even though no held press had started, so the list stayed at
index 0 for the whole run.

## Fix

- Preserve the benchmark step index when a scenario step does not run.
- Add a regression test for the summary-with-zero-rows held-scroll path.
- Fail velocity benchmarks when a required-motion scenario records zero moving
  frames.

## Hardware Evidence

Before:

- Command: `scripts/profile-preview-scroll.sh 60 held-scroll ITEM01-BEFORE-9c9ee441 --skip-build --visual-captures 0`
- Artifacts:
  - `build/preview-scroll-profiles/ITEM01-BEFORE-9c9ee441-arcade.tsv`
  - `build/preview-scroll-profiles/ITEM01-BEFORE-9c9ee441-arcade.log`
- Motion row: `frames=3598`, `fractional_visual_index_frames=0`,
  `moving_frames=0`.

After:

- Command: `scripts/profile-preview-scroll.sh 60 held-scroll ITEM01-AFTER-held-scroll-truth --deploy-device --visual-captures 0`
- Artifacts:
  - `build/preview-scroll-profiles/ITEM01-AFTER-held-scroll-truth-arcade.tsv`
  - `build/preview-scroll-profiles/ITEM01-AFTER-held-scroll-truth-arcade.log`
- Motion row: `frames=3597`, `fractional_visual_index_frames=3130`,
  `moving_frames=3577`.

## Validation

- `scripts/profile-preview-scroll.sh --self-test`
- `scripts/test-host-tools.sh`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features held_scroll_keeps_initial_press_when_summary_has_no_rows`
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
- `cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings`
