# Item 7 Slint Dirtiness Evidence - 2026-07-03

Scope: production Arcade launcher path, RGB565, turbo-hold benchmark. Experimental
effects excluded.

Confirmed cause:

- Locked Arcade scrolling used the light bridge sync path, but it still pushed
  `arcade_selected` and `arcade_scroll_y` into Slint for frames where Rust owns
  the direct-painted Arcade list and preview overlays.
- The startup splash visibility bridge property was also written every loop
  after startup although the value was unchanged.

Valid BEFORE artifacts:

- `build/arcade-scroll-profiles/ITEM7-BEFORE-20260703-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/ITEM7-BEFORE-20260703-arcade-scroll.log`
- `build/arcade-scroll-profiles/ITEM7-BEFORE-20260703-arcade-scroll.status.json`
- `build/preview-scroll-profiles/ITEM7-BEFORE-20260703-arcade.tsv`
- `build/preview-scroll-profiles/ITEM7-BEFORE-20260703-arcade.log`

Valid AFTER artifacts:

- `build/arcade-scroll-profiles/ITEM7-AFTER3-20260703-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/ITEM7-AFTER3-20260703-arcade-scroll.log`
- `build/arcade-scroll-profiles/ITEM7-AFTER3-20260703-arcade-scroll.status.json`
- `build/preview-scroll-profiles/ITEM7-AFTER3-20260703-arcade.tsv`
- `build/preview-scroll-profiles/ITEM7-AFTER3-20260703-arcade.log`

Excluded invalid AFTER attempts:

- `ITEM7-AFTER-20260703`
- `ITEM7-AFTER2-20260703`

Those attempts were run after accidentally deploying a non-bench-tools binary, so
`MISTER_LAUNCHER_BENCH_SCENARIO` was ignored and no trace was produced.

Metrics:

| Metric | BEFORE | AFTER | Result |
| --- | ---: | ---: | --- |
| Arcade scroll `slint_render_us` p95 | 303us | 282us | better |
| Arcade scroll `slint_render_us` p99 | 340us | 319us | better |
| Arcade scroll `rows` p95 | 704 | 704 | unchanged |
| Arcade scroll `cached_present_us` p99 | 0us | 0us | unchanged |
| Arcade scroll composition recovery | 0 | 0 | unchanged/pass |
| Preview guard `arcade_list_present_us` p95 | 570us | 557us | better |
| Preview guard `arcade_list_present_us` p99 | 601us | 580us | better |
| Preview guard `fb_present_us` p95 | 911us | 901us | better |
| Preview guard `fb_present_us` p99 | 943us | 927us | better |
| Preview guard `work_gt_16_7ms` | 0 | 0 | unchanged/pass |
| Preview guard exact frames | 1622 | 1620 | no blank/stale regression |

Validation:

- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features --lib`
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
- `scripts/dev-rust check`
