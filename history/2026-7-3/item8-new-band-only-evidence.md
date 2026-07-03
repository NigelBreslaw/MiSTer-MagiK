# Item 8 New-Band-Only Evidence - 2026-07-03

Scope: production Arcade launcher path, RGB565, turbo-hold benchmark. Experimental
effects excluded.

Confirmed cause:

- The Arcade list renderer already advances a cached RAM ring surface and draws
  only newly exposed content bands for scroll updates.
- Presenting only those newly exposed bands to `/dev/fb0` is not visually
  correct: the preserved rows would remain at their previous HDMI positions
  unless they are also copied to their new screen positions.
- The supported no-read source for those preserved rows is the RAM ring surface,
  so the visually correct no-`/dev/fb0`-read path still has to copy the moved
  visible rows from RAM to the framebuffer. That is effectively the current
  full overlay rewrite for the 464x384 list viewport.
- The code already documents the prior rejected path: `Scroll` is not a
  framebuffer dirty rect because a live-framebuffer scroll-present path was
  visually correct but slower due to expensive `/dev/fb0` reads.

Relevant source:

- `magik-gui/src/arcade_list_renderer.rs`: `ArcadeListUpdate::Scroll` marks RAM
  surface scroll reuse, not a framebuffer dirty rect.
- `magik-gui/src/ui_runner/ui_frame_target.rs`: scroll updates intentionally
  call `copy_layer_to_target(..., false)` and rewrite the direct list layer from
  RAM.
- `magik-gui/src/ui_runner/launcher_compositor.rs`: the Arcade list is a direct
  overlay excluded from cached Slint/base presents and then presented
  separately.
- `magik-gui/src/framebuffer/mapped.rs`: `/dev/fb0` is a single write-combining
  mapped buffer with write-side rect/row copies; no safe pan/blit primitive is
  available for moving old visible rows without reading or rewriting them.

Valid BEFORE artifacts:

- `build/arcade-scroll-profiles/ITEM8-BEFORE-20260703-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/ITEM8-BEFORE-20260703-arcade-scroll.log`
- `build/arcade-scroll-profiles/ITEM8-BEFORE-20260703-arcade-scroll.status.json`
- `build/preview-scroll-profiles/ITEM8-BEFORE-20260703-arcade.tsv`
- `build/preview-scroll-profiles/ITEM8-BEFORE-20260703-arcade.log`
- `build/preview-scroll-profiles/ITEM8-BEFORE-20260703-visuals/idx000.png`
- `build/preview-scroll-profiles/ITEM8-BEFORE-20260703-visuals/idx007.png`
- `build/preview-scroll-profiles/ITEM8-BEFORE-20260703-visuals/idx014.png`
- `build/preview-scroll-profiles/ITEM8-BEFORE-20260703-visuals/idx021.png`

Metrics:

| Metric | BEFORE | AFTER | Result |
| --- | ---: | ---: | --- |
| Arcade scroll `arcade_list_present_us` p95 | 637us | n/a | correctness-only |
| Arcade scroll `arcade_list_present_us` p99 | 702us | n/a | correctness-only |
| Arcade scroll `rows` p95 | 704 | n/a | correctness-only |
| Arcade scroll composition recovery | 0 | n/a | correctness-only |
| Preview guard `arcade_list_present_us` p95 | 557us | n/a | correctness-only |
| Preview guard `arcade_list_present_us` p99 | 588us | n/a | correctness-only |
| Preview guard `work_gt_16_7ms` | 0 | n/a | correctness-only |
| Preview visual captures | 4 nonblank | n/a | correctness-only |

Decision:

- No production code change was made for item 8.
- A guarded new-band-only present would be faster only by skipping necessary
  moved-row presentation; that would produce stale row positions.
- The correct production behavior remains the existing RAM-surface overlay
  rewrite until there is a safe hardware/driver primitive or a different
  double-buffered scan-out model.

Validation:

- `scripts/profile-arcade-scroll.sh ITEM8-BEFORE-20260703 --secs 30 --scenario turbo-hold --skip-build --thread-sample`
- `scripts/profile-preview-scroll.sh ITEM8-BEFORE-20260703 --skip-build --secs 30 --scenario turbo-hold --visual-captures 4 --replace-label`
- Source audit by main agent and explorer agent `019f2963-f10f-7bd0-adc8-8c442f0d3de4`.
