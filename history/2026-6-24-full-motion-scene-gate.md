# Item 16 - Full Motion Scene Gate

## Confirmed Cause

The scene gate was summing cumulative `fallback=` counters from normal UI FPS
logs as if they were per-window counters. `ui_frame_loop.rs` prints
`pacer.fallback_frames()` and `pacer.errors()`, which are cumulative. A single
startup fallback repeated across fourteen one-second FPS lines was therefore
reported as `vsync_fallback=14`.

The timing parser already discards the first three FPS windows as warmup. The
vsync gate now follows the same policy:

- cumulative logs: count the counter delta after the third FPS row.
- per-window profiler logs: sum only rows after the third FPS row.

This keeps startup pacer readiness out of the steady-state scene gate while
still failing real post-warmup fallback or error frames.

## Before

Historical failing artifact:

- `history/toolchain-bench/PERFREVIEW-20260624-SCENE-full_motion-ui.log`
- `history/toolchain-bench/PERFREVIEW-20260624-SCENE-full_motion-fb.png`
- `history/toolchain-bench/results.tsv`

Historical row:

```text
PERFREVIEW-20260624-SCENE full_motion ... visual_ok=no timing_ok=no capture_ok=yes ... vsync_fallback=14; vsync_errors=0; bench_failures=vsync-fallback=14>0,timing_ok=no,visual_ok=no
```

Log audit:

```text
sum=14 max=1 last=1
```

Fresh immediate-parent repro:

```bash
scripts/bench-toolchain.sh ITEM16-BEFORE-scene-gate --skip-build --device --replace-label --scene-secs 15
```

Before result:

```text
ITEM16-BEFORE-scene-gate demo ... visual_ok=no timing_ok=no capture_ok=yes ... vsync_fallback=14; vsync_errors=0; bench_failures=vsync-fallback=14>0,timing_ok=no,visual_ok=no
ITEM16-BEFORE-scene-gate full_motion ... visual_ok=yes timing_ok=yes capture_ok=yes ... vsync_fallback=0; vsync_errors=0
```

Fresh log audit for the failing `demo` row:

```text
ITEM16_BEFORE_demo sum=14 max=1 last=1
```

Single-scene sanity check on the same parent:

```bash
scripts/bench-toolchain.sh ITEM16-BEFORE-full-motion --device --replace-label --scene full_motion --scene-secs 15
```

```text
ITEM16-BEFORE-full-motion full_motion ... visual_ok=yes timing_ok=yes capture_ok=yes ... vsync_fallback=0; vsync_errors=0
```

## After

```bash
scripts/bench-toolchain.sh ITEM16-AFTER-scene-gate --skip-build --device --replace-label --scene-secs 15
```

After result:

```text
ITEM16-AFTER-scene-gate demo ... visual_ok=yes timing_ok=yes capture_ok=yes ... vsync_fallback=0; vsync_errors=0
ITEM16-AFTER-scene-gate full_motion ... visual_ok=yes timing_ok=yes capture_ok=yes ... vsync_fallback=0; vsync_errors=0
ITEM16-AFTER-scene-gate static_ui ... visual_ok=yes timing_ok=yes capture_ok=yes ... vsync_fallback=0; vsync_errors=0
ITEM16-AFTER-scene-gate local_motion ... visual_ok=yes timing_ok=yes capture_ok=yes ... vsync_fallback=0; vsync_errors=0
```

Metric:

- `full_motion` historical gate: `vsync_fallback 14 -> 0`, `timing_ok no -> yes`,
  `visual_ok no -> yes`, `capture_ok yes -> yes`.
- Fresh all-scenes gate: first-scene false failure `vsync_fallback 14 -> 0`,
  all scenes pass with `timing_ok=yes`, `visual_ok=yes`, `capture_ok=yes`.

## Validation

```bash
bash -n scripts/bench-toolchain.sh
scripts/bench-toolchain.sh --self-test
scripts/test-host-tools.sh
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```

All passed.
