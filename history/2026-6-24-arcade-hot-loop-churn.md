# Arcade Hot-Loop Churn

Item 6 of the production performance plan.

## Confirmed Cause

Three small UI-thread costs were present on every Arcade frame:

- `ArcadeListRenderer::draw` built the visible-window content hash before it
  knew whether the frame was a same-position frame or a motion frame. That
  walked and byte-hashed every visible row even though motion frames only need
  the list length, anchor identity, and pixel delta.
- Preview scheduling could run once from bridge sync and again from prepare on
  the same logical loop. Worker/pending dedupe prevented duplicate decode jobs,
  but the UI thread still rescanned the same prefetch window.
- Runtime status strings were copied from Slint every frame even though
  `write_runtime_status` returns immediately unless the once-per-second status
  write is due. The old trace only timed title/detail copies, so it under-
  reported the real copy surface.

## Fix

- Cache per-row Arcade identity fingerprints keyed by row index. The fingerprint
  still covers the same fields as the old hash: system id, MRA path, preview
  archive path, preview asset key, title, and `is_new`.
- Skip visible-window hashing during scroll motion frames. Same-position frames
  still compute the visible hash, but use cached row fingerprints.
- Track whether the bridge already scheduled the Arcade preview window in the
  current loop and skip the later prepare-time schedule when it did.
- Cache the last prefetch window and skip rescanning it when every candidate is
  already cached, failed, or pending.
- Snapshot status strings only when a status write is due. The trace byte count
  now includes all status strings in that due-frame snapshot, not just catalog
  title/detail.

## Benchmarks

Immediate-parent before evidence:

```text
scripts/profile-preview-scroll.sh 60 turbo-hold ITEM06-BEFORE-hot-loop --skip-build --visual-captures 0
```

After evidence:

```text
scripts/deploy-rust.sh --device --ui-scope launcher
scripts/profile-preview-scroll.sh 60 turbo-hold ITEM06-AFTER-hot-loop --skip-build --visual-captures 0
```

| Metric | Before | After | Result |
| --- | ---: | ---: | --- |
| `arcade_list_update_us` avg | 180.57 us | 140.07 us | 22.4% lower |
| `arcade_list_update_us` p95 | 534 us | 483 us | 9.6% lower |
| `arcade_list_update_us` p99 | 629 us | 557 us | 11.4% lower |
| `preview_schedule_us` avg | 177.68 us | 80.86 us | 54.5% lower |
| `preview_schedule_us` p99 | 197 us | 182 us | 7.6% lower |
| `runtime_status_write_us` p99 | 821 us | 794 us | 3.3% lower |
| `work_gt_16.7ms` | 10 frames | 0 frames | eliminated |

Steady scroll update mix stayed equivalent:

```text
Before: full=2, scroll:-12=3563, scroll:12=12, none=21
After:  full=2, scroll:-12=3563, scroll:12=13, none=20
```

Preview work stayed healthy:

```text
Before: decoded=907 apply=7, selected_decode_queue_age_us p95=42675, prefetch_decode_queue_age_us p95=2450
After:  decoded=909 apply=9, selected_decode_queue_age_us p95=32119, prefetch_decode_queue_age_us p95=2246
```

Status snapshot note:

- Before `status_string_copy_bytes` was under-reported; it only counted
  catalog title/detail and showed total=0 after warmup.
- After the metric counts all status strings, but only on the once-per-second
  due frames: nonzero frames=58, total=4350 bytes over 3567 post-warmup frames.
- Non-due frames no longer copy the status-only strings.

Artifacts:

- `build/preview-scroll-profiles/ITEM06-BEFORE-hot-loop-arcade.tsv`
- `build/preview-scroll-profiles/ITEM06-BEFORE-hot-loop-arcade.log`
- `build/preview-scroll-profiles/ITEM06-BEFORE-hot-loop-arcade.status.txt`
- `build/preview-scroll-profiles/ITEM06-AFTER-hot-loop-arcade.tsv`
- `build/preview-scroll-profiles/ITEM06-AFTER-hot-loop-arcade.log`
- `build/preview-scroll-profiles/ITEM06-AFTER-hot-loop-arcade.status.txt`

## Validation

```text
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features arcade_list_renderer
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features preview_state
cargo check --manifest-path magik-gui/Cargo.toml --features ui --no-default-features
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path magik-gui/Cargo.toml --check
```
