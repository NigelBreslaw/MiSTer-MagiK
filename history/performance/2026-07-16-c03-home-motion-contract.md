# Commit 03: truthful Home motion traces

## Scope and confirmed cause

Correctness-only benchmark instrumentation change; no performance claim.

The Home max-scroll gate previously evaluated latch health while the trace's
`selected` and `visual_index` fields always described Arcade navigation. The
analyzer did not require Home selection, repaint, scroll, or pan evidence, so a
healthy idle launcher could pass as successful Home motion.

Immediate parent: `12d941480d0c3366c61514d07a8bc6bd0ee6a92e`.

## BEFORE contract

Command (candidate binary was built and deployed from the immediate parent
before the timed run):

```text
scripts/gate-launcher-home-max-scroll-zero-drops.sh PERF-C03-HOME-MOTION-12D94148-BEFORE --secs 30 --menu consoles --skip-build
```

Result: exit 0 and `valid=1` across 1,763 measured frames, despite
`selected_distinct=1` and `visual_index_distinct=1` (both Arcade zero). The
underlying trace contained 1,180 `home_pan_present_active` frames, but no
Home-native identity, selected index, scroll position, or extent fields, and no
Home motion acceptance check.

Raw artifacts:

- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-BEFORE-launcher-home-scroll.tsv`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-BEFORE-launcher-home-scroll-drops.tsv`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-BEFORE-launcher-home-scroll.log`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-BEFORE-launcher-home-scroll.status.json`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-BEFORE-fpga-latch-before.log`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-BEFORE-fpga-latch-after.log`

## AFTER contract

Command (candidate binary built and deployed before the timed run):

```text
scripts/gate-launcher-home-max-scroll-zero-drops.sh PERF-C03-HOME-MOTION-12D94148-AFTER-PARSER --secs 30 --menu consoles --skip-build
```

Result: exit 0, `valid=1`, and `home_motion_valid=1` across 1,762 measured
frames. The nested-pan contract observed one stable Home menu, seven selected
identities/indexes, scroll `0..509` for extent 509, 1,173 pan-active frames,
842 scroll-change/pan frames, both endpoints, and 30 direction reversals.
Presentation remained on `fpga-vblank-latch-hidden`, with zero latch deadline
misses, zero visual latch misses, zero FPGA drops, and no over-budget frame
work. This is a correctness result only; no performance claim is made.

Raw artifacts:

- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-AFTER-PARSER-launcher-home-scroll.tsv`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-AFTER-PARSER-launcher-home-scroll-drops.tsv`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-AFTER-PARSER-launcher-home-scroll.log`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-AFTER-PARSER-launcher-home-scroll.status.json`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-AFTER-PARSER-fpga-latch-before.log`
- `build/launcher-home-scroll-profiles/PERF-C03-HOME-MOTION-12D94148-AFTER-PARSER-fpga-latch-after.log`

## Validation

- `scripts/analyze-max-scroll-drops.py --self-test` (includes a healthy idle
  Home trace that must fail, plus root-focus and nested-pan positive cases)
- `scripts/analyze-arcade-frame-trace.py --self-test` (accepts the shared
  trace's new string-valued `home_screen` column)
- `bash -n scripts/gate-launcher-home-max-scroll-zero-drops.sh`
- `python3 -m py_compile scripts/analyze-max-scroll-drops.py`
- `cargo fmt --manifest-path magik-gui/Cargo.toml -- --check`
- `scripts/dev-rust test` (283 passed)
- `scripts/dev-rust check`
- `cargo check --manifest-path magik-gui/Cargo.toml --features ui,bench-tools,diagnostics --no-default-features`
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
- `cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings`

## Review resolution

The first final review found that Home identity hashing was compiled and run in
ordinary production UI frames despite having only benchmark/diagnostic
consumers. The trace fields, accessors, token hashing, and frame extraction are
now feature-gated by `bench-tools`/`diagnostics`. The validation above was rerun
for both ordinary and instrumented feature sets, and the replacement AFTER run
above supersedes the earlier candidate run.

Post-commit verification of the shared Arcade trace consumer exposed that its
generic parser still treated the new string-valued `home_screen` column as an
integer. The parser and its round-trip self-test now cover that shared-schema
field; the hardware trace payload itself is unchanged.

The change is limited to production benchmark/correctness instrumentation and
Home navigation accessors. It does not touch experimental effects or reboot and
fault-injection paths.
