# Preview Trace Overhead

Item: reduce preview trace overhead.

## Confirmed Cause

`LauncherFrameAccounting::write_preview_trace` formatted and wrote one TSV row
to the FAT-backed trace file every frame, then flushed the file every frame.
The supervised Arcade turbo-hold CPU profile attributed 46 samples, 4.96%, to
`write_preview_trace`, with the samples dominated by `std::io::Write::write_fmt`
and file writes.

## Fix

Preview scroll tracing now records compact scalar rows in memory during the
frame loop. TSV serialization is deferred until the trace closes or the trace
object drops. The writer is buffered, and long-running manual traces flush in
4096-row batches so an unbounded trace environment cannot grow memory forever.

The TSV header and row format are unchanged.

## Benchmark

BEFORE command:

```bash
scripts/profile-preview-scroll.sh 30 turbo-hold ITEM03-BEFORE-trace-overhead --cpu-profile --visual-captures 0
```

BEFORE artifacts:

- `build/preview-scroll-profiles/ITEM03-BEFORE-trace-overhead-arcade.tsv`
- `build/preview-scroll-profiles/ITEM03-BEFORE-trace-overhead-arcade.log`
- `build/preview-scroll-profiles/ITEM03-BEFORE-trace-overhead-arcade.status.txt`
- `build/preview-scroll-profiles/ITEM03-BEFORE-trace-overhead-arcade-cpu.svg`

BEFORE metrics:

- `write_preview_trace`: 46 samples, 4.96%.
- `p99_work_us`: 14672.
- Motion validity: `moving_frames=1779`, `fractional_visual_index_frames=1335`.

AFTER command:

```bash
scripts/profile-preview-scroll.sh 30 turbo-hold ITEM03-AFTER-deferred-trace-v2 --deploy-device --cpu-profile --visual-captures 0
```

AFTER artifacts:

- `build/preview-scroll-profiles/ITEM03-AFTER-deferred-trace-v2-arcade.tsv`
- `build/preview-scroll-profiles/ITEM03-AFTER-deferred-trace-v2-arcade.log`
- `build/preview-scroll-profiles/ITEM03-AFTER-deferred-trace-v2-arcade.status.txt`
- `build/preview-scroll-profiles/ITEM03-AFTER-deferred-trace-v2-arcade-cpu.svg`

AFTER metrics:

- `write_preview_trace`: 1 sample, 0.11%.
- `p99_work_us`: 14737.
- Motion validity: `moving_frames=1778`, `fractional_visual_index_frames=1334`.

The item target was `<1%` CPU samples in the preview trace writer. The measured
writer cost dropped from 4.96% to 0.11%, with no meaningful p99 work regression.

## Validation

```bash
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features held_scroll_keeps_initial_press_when_summary_has_no_rows
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```
