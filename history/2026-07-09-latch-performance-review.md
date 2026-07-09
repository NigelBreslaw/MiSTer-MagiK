# 2026-07-09 Latch Performance Review

This note captures the expanded multi-agent performance review of the FPGA
vblank latch renderer. Scope is strictly latch rendering performance, not
MiSTer MagiK as a whole.

## Review Shape

Six read-only subagents reviewed the latch system:

- 2 static-code reviewers:
  - memory/copy hot path
  - pacing/FPGA command hot path
- 2 data-only reviewers:
  - latch metric synthesis
  - tail/outlier analysis
- 2 combined code plus data reviewers:
  - practical optimization plan
  - adversarial review to challenge the obvious conclusions

The main thread also ran the device benchmarks and checked key claims against
the produced TSV files.

## Benchmarks Run

Device benchmark labels:

- Home hard left/right scroll:
  - `LATCH-REVIEW-20260709-HOME`
  - Command: `scripts/gate-launcher-home-max-scroll-zero-drops.sh LATCH-REVIEW-20260709-HOME --secs 30 --deploy-device`
- Arcade held scroll:
  - `LATCH-REVIEW-20260709-ARCADE-HELD`
  - Command: `scripts/profile-arcade-scroll.sh LATCH-REVIEW-20260709-ARCADE-HELD --secs 30 --scenario held-scroll --skip-build`
- Arcade turbo hold:
  - `LATCH-REVIEW-20260709-ARCADE-TURBO`
  - Command: `scripts/profile-arcade-scroll.sh LATCH-REVIEW-20260709-ARCADE-TURBO --secs 30 --scenario turbo-hold --skip-build`
- Arcade human turbo hold:
  - `LATCH-REVIEW-20260709-ARCADE-HUMAN`
  - Command: `scripts/profile-arcade-scroll.sh LATCH-REVIEW-20260709-ARCADE-HUMAN --secs 30 --scenario human-turbo-hold --skip-build`

Artifacts:

- `build/launcher-home-scroll-profiles/LATCH-REVIEW-20260709-HOME-*`
- `build/arcade-scroll-profiles/LATCH-REVIEW-20260709-ARCADE-HELD-*`
- `build/arcade-scroll-profiles/LATCH-REVIEW-20260709-ARCADE-TURBO-*`
- `build/arcade-scroll-profiles/LATCH-REVIEW-20260709-ARCADE-HUMAN-*`

## Top-Level Conclusions

The latch mechanism itself looks stable in the Home gate:

- Home gate passed: `valid=1`.
- Home latch rows: `1764` in the drops summary.
- Home backend/status: all `fpga-vblank-latch-hidden` / `ok`.
- Home latch deadline misses: `0`.
- Home visual latch misses: `0`.
- Home buffer alternation failures: `0`.
- Home FPGA drop count max: `0`.
- Home FPGA counters advanced: `flip_delta=2448`, `post_delta=2449`.
- Home latch margin min: `5745 us`.

Arcade traces prove the latch backend is active by trace status:

- All three Arcade TSVs report `main_present_backend=fpga-vblank-latch-hidden`.
- All three Arcade TSVs report `main_present_status=ok` on every row.
- Buffer alternation is clean in the Arcade TSVs.

But Arcade lacks the same passive before/after FPGA counter proof as Home:

- There are no matching `LATCH-REVIEW-20260709-ARCADE-*-fpga-latch-before.log`
  and `*-after.log` files.
- Future Arcade latch reviews should capture passive `fpga-latch-report`
  before and after each run, matching the Home gate.

The first simple conclusion, "full-frame copy is the main thing", was too
simple. Full-frame hidden copy is a real constant tax and has rare large spikes,
but the worst wall-time tails in these runs are mostly scheduler/vsync cadence,
not latch copy or FPGA post/status. Home smoothness is more strongly affected by
Slint render cost than hidden copy. Arcade steady-state cost is a combination of
full hidden copy plus preview/list composition work.

## Latch Health By Scenario

| Scenario | Frames | Backend/status | Derived latch deadline misses |
| --- | ---: | --- | ---: |
| Home | 1795 TSV / 1764 drops summary | 1795/1795 ok | 0 |
| Arcade held | 1799 | 1799/1799 ok | 1, first timed row only |
| Arcade turbo | 1799 | 1799/1799 ok | 1, first timed row only |
| Arcade human | 1799 | 1799/1799 ok | 1, first timed row only |

The one derived Arcade miss in each file is frame `94`, where `elapsed_us=0`.
This looks like timed-window boundary or pacer phase state, not a steady-state
latch renderer miss.

Derived first-row Arcade misses:

| Scenario | Frame | present_phase_us | copy_us | request_us | wait_us | Derived margin |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Held | 94 | 23537 | 1740 | 23 | 0 | -8633 us |
| Turbo | 94 | 23376 | 1539 | 24 | 0 | -8272 us |
| Human | 94 | 20992 | 1432 | 24 | 0 | -5781 us |

Next rows return to normal phase:

- Held frame 95: `present_phase_us=2891`, `wall_us=16641`.
- Turbo frame 95: `present_phase_us=3126`, `wall_us=16657`.
- Human frame 95: `present_phase_us=948`, `wall_us=16637`.

After the first Arcade row, worst derived 16.667 ms margins remain healthy:

- Held: `5431 us`.
- Turbo: `7922 us`.
- Human: `6901 us`.

## Benchmark Gate Results

Home gate:

```text
max_scroll_gate_tsv label=LATCH-REVIEW-20260709-HOME valid=1
latch_deadline_misses=0 visual_latch_misses=0 fpga_drop_count_max=0
latch_margin_min=5745 latch_margin_p50=8525 latch_margin_p99=13336
```

Arcade pacing gate rows failed, but due to wall-clock cadence rather than work
over budget:

| Scenario | valid | p99_work_us | work_gt_16667 | p99_wall_us | max_wall_us | low_work_high_wall | max_miss_streak |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Held | 0 | 4818 | 0 | 16962 | 33225 | 157 | 0 |
| Turbo | 0 | 6070 | 0 | 17315 | 33326 | 174 | 0 |
| Human | 0 | 5887 | 0 | 17322 | 33361 | 186 | 0 |

Important caveat: the current Arcade gate is not latch-deadline-aware in the
same way as the Home gate. It also does not emit a drops report for these exact
Arcade labels.

## Full-Frame Hidden Copy Tax

Every reviewed run copied the full 960x540 RGB565 frame every presented frame:

- Rows: `540` on every frame.
- Bytes: `1,036,800` on every frame.
- Code path:
  - `magik-gui/src/ui_runner/launcher_loop.rs:3325`
  - `magik-gui/src/ui_runner/launcher_loop.rs:213`
  - `magik-gui/src/framebuffer/plugin_probe.rs:290`

Hidden copy timings:

| Scenario | p50 | p95 | p99 | max |
| --- | ---: | ---: | ---: | ---: |
| Home | 1476 us | 1658 us | 1727 us | 2072 us |
| Held | 1534 us | 2019 us | 2233 us | 10296 us |
| Turbo | 1589 us | 2038 us | 2271 us | 3836 us |
| Human | 1578 us | 2050 us | 2389 us | 7005 us |

Outliers over `3000 us`:

- Home: `0`.
- Held: `4`.
- Turbo: `1`.
- Human: `4`.

Representative copy outliers:

```text
HELD frame 434: hidden_copy=10296 us, wall=16672 us, vsync=3526 us
HUMAN frame 460: hidden_copy=7005 us, wall=16652 us, vsync=4156 us
TURBO frame 721: hidden_copy=3836 us, wall=16273 us, vsync=7943 us
HOME frame 1565: hidden_copy=2072 us, wall=16638 us
```

Interpretation:

- Full-frame copy is a real constant tax.
- Copy outliers are large enough to investigate.
- In these data, copy outliers usually had enough vsync slack and did not cause
  the worst wall frames.

## FPGA Post, Status, And Wait Cost

FPGA request cost is small in these runs:

| Scenario | request p50 | request p95 | request p99 | request max |
| --- | ---: | ---: | ---: | ---: |
| Home | 22 us | 26 us | 49 us | 191 us |
| Held | 24 us | 29 us | 55 us | 269 us |
| Turbo | 24 us | 29 us | 55 us | 205 us |
| Human | 24 us | 29 us | 58 us | 469 us |

Latch status/wait cost is also usually tiny:

| Scenario | wait p99 | wait max |
| --- | ---: | ---: |
| Home | 13 us | 13 us |
| Held | 13 us | 16 us |
| Turbo | 13 us | 16 us |
| Human | 13 us | 187 us |

Static code findings still matter:

- `post_magik_latched_fbuf_rgb565()` parses `MISTER_FB_RIGHT_GUARD_COLS` in the
  per-frame post path.
  - Site: `magik-gui/src/fpga.rs:409`.
- `post_magik_latched_fbuf_rgb565()` can call `set_vga_fb(true)` every frame
  when `set_vga_fb` is true.
  - Site: `magik-gui/src/fpga.rs:436`.
- Status polling reads command plus 11 words every default 60 frames.
  - Sites:
    - `magik-gui/src/ui_runner/launcher_loop.rs:240`
    - `magik-gui/src/ui_runner/launcher_loop.rs:284`
    - `magik-gui/src/fpga.rs:366`

Recommended changes:

- Cache route geometry and right guard columns outside the per-frame post.
- Split `main_present_request_us` into latch post and optional `set_vga_fb_us`.
- Move `set_vga_fb(true)` to presenter open, route recovery, or explicit route
  reassertion, if stable in direct-video mode.
- Make status polling diagnostics-only or idle-time if later data shows it
  matters. In these runs it is not the dominant cost.

## Worst Wall-Time Tails

The worst wall frames are dominated by `vsync_us`, not latch copy/request/wait.

Examples:

```text
HOME frame 0: wall=27395 us, startup/status frame
  slint_render=12885, runtime_status_write=1155, frame_finish=1974
  hidden_copy=1887, wait=13

HOME frame 57: wall=17068 us
  vsync=12478, hidden_copy=1873, request=26, wait=0, status_due=0

HELD frame 227: wall=33225 us, update=scroll:-6, preview_rows=320
  vsync=27696, hidden_copy=1617, request=26, wait=0, status_due=0

TURBO frame 406: wall=33326 us, update=scroll:-12, preview_rows=320
  vsync=26383, hidden_copy=1894, request=24, wait=0, status_due=0

HUMAN frame 834: wall=33361 us, update=scroll:-12, preview_rows=320
  vsync=27745, hidden_copy=1406, request=25, wait=0, status_due=0
```

Adjacent-frame pattern confirms cadence misses:

```text
HELD frames 226/227/228 wall: 16675 -> 33225 -> 16634
TURBO frames 405/406/407 wall: 16650 -> 33326 -> 16640
HUMAN frames 833/834/835 wall: 16640 -> 33361 -> 16611
```

Recommendation:

- Add per-frame vblank sequence or edge timing so a 33 ms wall frame can say
  explicitly whether one vblank was missed, even when `vsync_miss_streak=0`.

## Runtime Status And Frame Finish Tail

Runtime status writes explain most `frame_finish_us` spikes:

```text
TURBO frame 642: runtime_status_write=6207 us, frame_finish=6284 us, wall=16621 us
HOME frame 0: runtime_status_write=1155 us, frame_finish=1974 us, wall=27395 us
HELD frame 94: runtime_status_write=1027 us, frame_finish=2051 us, present_phase=23537 us
HUMAN frame 94: runtime_status_write=903 us, frame_finish=1717 us, present_phase=20992 us
```

Non-status max finish times are much lower:

- Home: `342 us`.
- Held: `284 us`.
- Turbo: `174 us`.
- Human: `165 us`.

Status-write max finish times:

- Home: `1974 us`.
- Held: `2051 us`.
- Turbo: `6284 us`.
- Human: `1717 us`.

Recommendation:

- Move or throttle runtime status writes off the hot latch frame tail.
- Log `slack_us = period_us - present_phase_us` against `frame_finish_us`,
  `runtime_status_write_us`, `vsync_accepted_hit_age_us`, and
  `vsync_stale_hits`.
- If slack is low after latch post, wait/observe vblank immediately and defer
  status/accounting.

## Home-Specific Finding

Home hard scroll is mostly Slint render, not hidden copy:

- Home `slint_render_us p50` is about `6435 us`.
- Home hidden copy p50 is about `1476 us`.
- Home worst margin still remains positive: `5745 us`.

Representative Home worst-margin frame:

```text
present_phase=9032 us
copy=1862 us
slint_render=8652 us
margin=5745 us
```

Static site:

- Home dirty expansion:
  - `magik-gui/src/ui_runner/launcher_loop.rs:1048`
- Home Slint view:
  - `magik-gui/ui/views/home.slint:56`

Recommendation:

- Full-frame hidden copy reduction will help Home, but only modestly.
- A bigger Home win is likely a custom RGB565 home carousel/pan surface, keeping
  Slint chrome static and custom-blitting the tile band during hard pan.
- This is a larger UI ownership change and needs visual validation.

## Arcade-Specific Finding

Arcade shifts cost away from Slint render and into preview/list work plus hidden
copy:

| Scenario | prepare p50 | custom_draw p50 | preview_blit p50 | hidden compose/direct present p50 |
| --- | ---: | ---: | ---: | ---: |
| Held | 211 us | 1432 us | 1397 us | 1926 us |
| Turbo | 1134 us | 1427 us | 1376 us | 2061 us |
| Human | 1103 us | 1418 us | 1358 us | 2061 us |

Direct preview rows:

- Home: `0` rows on `1795/1795` frames.
- Held: `320` rows on `1760/1799` frames.
- Turbo: `320` rows on `1788/1799` frames.
- Human: `320` rows on `1738/1799` frames.

The latch path currently composes direct preview and arcade list updates back
into cached RAM, then copies cached RAM to the hidden buffer:

- Direct preview compose site:
  - `magik-gui/src/ui_runner/launcher_loop.rs:3311`
- Arcade list compose site:
  - `magik-gui/src/ui_runner/launcher_loop.rs:3318`
- Arcade renderer site:
  - `magik-gui/src/arcade_list_renderer.rs:590`

Recommendation:

- After measurement cleanup, consider direct-to-hidden overlay writes:
  - copy cached Slint/base damage first;
  - copy direct preview rect into the selected hidden buffer;
  - copy arcade list update into the selected hidden buffer;
  - preserve layer order explicitly.
- This could save the extra cached staging cost in Arcade, around `1.9-2.1 ms`
  median, but correctness risk is real.

## Trace Semantics Problems

Latch mode currently records the same `hidden_compose_us` into both:

- `direct_preview_present_us`
- `arcade_list_present_us`

Site:

- `magik-gui/src/ui_runner/launcher_loop.rs:3337`

This makes attribution muddy. Do not sum those two fields in latch mode.

Recommended trace cleanup:

- Add `hidden_compose_us`.
- Split into `hidden_preview_compose_us` and `hidden_arcade_compose_us` if
  possible.
- Keep `main_present_hidden_copy_us`, `main_present_request_us`, and
  `main_present_wait_us` as separate fields.
- Add copied bytes/rect count for hidden buffer copy.

## Dirty Hidden Copy Risks

Partial dirty hidden-buffer copy is attractive but risky. Current safety comes
from copying the complete cached frame into the next alternating hidden slot.

If dirty-only copy is introduced, each hidden slot must receive every dirty
region it missed while inactive. It is not enough to copy only the current
frame's dirty rect into the selected hidden buffer.

Specific risk:

- Buffer 1 and buffer 2 can diverge and produce alternating stale pixels.
- Arcade can have `dirty_rect=none` while direct preview/list composition
  changes the cached frame.
- Overlay damage must be included in each hidden buffer's catch-up list.

Recommended implementation shape:

- Each hidden buffer keeps an invalid rect list, initially full frame.
- On each frame, compute all damage that changes the final presented image:
  - Slint dirty or full-frame dirty;
  - raw preview cached damage;
  - raw preview direct damage;
  - arcade overlay damage;
  - any forced full-frame or route recovery damage.
- Before posting buffer N, copy buffer N's invalid rect list from the final
  composited source into hidden buffer N.
- Clear buffer N's invalid list after successful copy/post.
- Add this frame's damage to the other hidden buffer's invalid list.
- Force both buffers invalid/full on route changes, geometry changes, fallback
  recovery, and first activation.

Validation:

- Add trace fields:
  - `hidden_invalid_bytes`
  - `hidden_rect_count`
  - `hidden_catchup_bytes`
  - `hidden_full_copy`
  - actual copied bytes and rows
- Compare full-copy latch versus dirty-copy latch on:
  - Home hard scroll;
  - Arcade held;
  - Arcade turbo;
  - Arcade human turbo.
- Require:
  - no visual latch misses;
  - no FPGA drops;
  - no buffer alternation failures;
  - no stale alternating pixels;
  - present bytes median below full-frame when damage is partial.

## Contiguous Full-Copy Fast Path

Static reviewers found a low-risk optimization:

- Normal launcher geometry uses source stride = destination stride = width.
- `PluginHiddenRgb565Framebuffer::copy_full_frame()` still loops over 540 rows.

Site:

- `magik-gui/src/framebuffer/plugin_probe.rs:307`
- `magik-gui/src/framebuffer/plugin_probe.rs:311`

Recommended change:

- If `src_stride_pixels == self.stride_pixels && self.stride_pixels == self.width`,
  do one contiguous slice copy.
- Keep the row path for non-contiguous geometry.

Expected impact:

- Low to medium.
- It will not remove memory bandwidth cost, but should reduce loop/slice overhead
  and give the compiler/libc a cleaner bulk-copy shape.

Validation:

- Add a plugin-hidden copy bench variant for row-loop versus contiguous copy.
- Report wall time, CPU time, and MB/s over at least 240 alternating-buffer
  frames.

## Plugin Hidden Copy Benchmark Gap

Static reviewers flagged a measurement gap:

- Existing diagnostic hidden copy bench uses `HiddenRgb565Framebuffer` via
  `/dev/mem`.
- Launcher latch uses `PluginHiddenRgb565Framebuffer`.

Sites:

- Diagnostic bench area:
  - `magik-gui/src/main.rs:800`
- Launcher plugin hidden open:
  - `magik-gui/src/ui_runner/launcher_loop.rs:135`

Because cache attributes and mapping behavior are central to hidden-buffer
performance, the existing bench may not represent the launcher path.

Recommendation:

- Add a bench mode using `PluginHiddenRgb565Framebuffer::open`.
- Report plugin write-combined hidden copy MB/s beside `/dev/mem` hidden copy
  MB/s.

## Present Byte Accounting Gap

Static reviewers found that present byte accounting may not reflect actual
broadened row copies:

- `cached_present_rect()` can broaden a dirty rect to full-width rows.
- The compositor records bytes for the original rect in some paths.

Sites:

- `magik-gui/src/framebuffer/target.rs:237`
- `magik-gui/src/framebuffer/target.rs:363`
- `magik-gui/src/ui_runner/launcher_compositor.rs:154`

Recommendation:

- Return actual copied rect/bytes from `present_cached_rect`.
- Log requested bytes and actual copied bytes.
- Use actual bytes to validate dirty-latch savings.

## Preview Fade During Fast Arcade Scroll

Combined review found a non-latch-subsystem but latch-path-adjacent opportunity:

- Arcade scroll spends about `1.3-1.4 ms` median in `preview_blit_us`.
- `preview_fade_cpu_us` is nearly the same.
- Most fast-scroll frames are on `rows`/`cut` fade paths while selection changes
  every frame.

Suggested experiment:

- During active fast scroll, hold previous preview, skip fade progression, or
  snap only after a settle window.
- Do not apply to ordinary single-step navigation.

Potential effect:

- Save about `1.0-1.6 ms/frame` CPU in turbo and human-turbo Arcade scroll.

Risk:

- Changes visual behavior.
- Needs validation that preview exactness and feel remain acceptable.

## Latch Late-Start And Idle Pacing Policy

Static pacing review flagged two policy risks.

### Conservative Late-Start Threshold

`FPGA_LATCH_LATE_FRAME_START_HEADROOM_US = 12_000`, so at 60 Hz a loop starting
after about `4.667 ms` waits before rendering.

Sites:

- `magik-gui/src/ui_runner/launcher_pacing.rs:29`
- `magik-gui/src/ui_runner/launcher_pacing.rs:55`
- `magik-gui/src/ui_runner/launcher_loop.rs:3193`

Recommendation:

- Base threshold on observed p95 render + hidden copy + latch post + guard,
  instead of fixed `12_000 us`.

### Idle Scheduler Stale Phase

Idle uses `thread::sleep(period)` instead of keeping the pacer last-hit fresh.
On the next redraw, `age_since_last_hit_us()` can be huge, forcing a pre-render
wait and shifting latch timing.

Sites:

- `magik-gui/src/ui_runner/launcher_loop.rs:3124`
- `magik-gui/src/ui_runner/launcher_loop.rs:3180`
- `magik-gui/src/framebuffer/vsync.rs:243`

Recommendation:

- Measure first active frame after idle:
  - `last_frame_ms_ago`
  - `frame_start_phase_us`
  - `pre_render_wait_us`
- Consider treating stale phase as unknown/modulo-period, or use pacer waits at
  a low rate while idle.

## Trace Deferral And Allocation

Latch mode defers trace flushes to avoid hot-path I/O, which is good. Reviewers
still flagged possible allocation/copy bursts if trace rows grow beyond initial
capacity.

Sites:

- `magik-gui/src/ui_runner/launcher_loop.rs:3508`
- `magik-gui/src/ui_runner/launcher_loop.rs:3653`
- `magik-gui/src/ui_runner/launcher_frame_accounting.rs:441`

Data:

- `post_finish_tail_us` p99 is small:
  - Home: `5 us`
  - Held: `9 us`
  - Turbo: `7 us`
  - Human: `7 us`

Recommendation:

- Low priority.
- Pre-reserve trace rows by trace duration or use a fixed ring/background flush
  if longer latch traces show tail spikes.

## Prioritized Next Work

1. **Measurement correctness first**
   - Add Arcade passive `fpga-latch-report` before and after.
   - Produce Arcade drops reports equivalent to Home.
   - Handle or ignore the first `elapsed_us=0` post-entry row in Arcade latch
     deadline analysis.
   - Split latch trace attribution fields.

2. **Low-risk copy fast path**
   - Add contiguous full-frame copy path in `PluginHiddenRgb565Framebuffer`.
   - Add plugin-hidden copy microbench.

3. **Frame tail cleanup**
   - Move or throttle runtime status writes out of low-slack latch frames.
   - Add explicit slack and vblank edge diagnostics.

4. **Dirty hidden copy prototype**
   - Implement per-hidden-buffer dirty catch-up only after trace fields can prove
     copied bytes, rect counts, and correctness.
   - Validate hard against alternating stale pixels.

5. **Direct-to-hidden overlays**
   - Avoid cached staging for direct preview/list overlays in latch mode.
   - Preserve layer order and stream/debug consistency.

6. **Scenario-specific improvements**
   - Home: custom RGB565 carousel/tile-band renderer if Home remains a target.
   - Arcade: fast-scroll preview fade suppression or settle-window behavior.

## Do Not Overclaim

Do not claim from this data that:

- Arcade has passive FPGA drop-count proof. It does not for these labels.
- Full-frame hidden copy is the only or proven dominant bottleneck. It is a real
  constant tax and has rare spikes, but worst wall tails here are cadence/vsync.
- Arcade `work_gt_16667=0` means all latch-adjacent work is under budget. The
  current trace attribution double-counts or misattributes hidden compose in
  latch mode.
- Dirty hidden copy is straightforward. Double buffering makes correctness the
  hard part.

