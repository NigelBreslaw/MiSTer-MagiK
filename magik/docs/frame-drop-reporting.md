# Frame-drop evidence

Native measurement windows report owned refresh repeats while motion is active,
separately from latch rejections and missing card-source poses. A workload label
identifies activity when a repeat was observed; it does not prove its cause.
`dropped_frames_by_workload` covers every recorded event, including events beyond
the bounded 64-record detail buffer. `dropped_frame_records_omitted` counts omitted
records, not omitted refreshes. A record can cover more than one refresh.

Motion starting from rest has no earlier refresh to meet: input can arrive
anywhere in a period, so its first frame may wait one refresh for scanout. That
wait is input latency, not a drop. Windows report it as
`first_frame_wait_refreshes` beside `motion_starts`. Repeats the first frame's
own work would cause from a refresh boundary still count as dropped, as does
every repeat after that first frame.

`ui_render_us`, `last_render_us` and the window `render_*` statistics cover the
existing render interval **before custom drawing**. This includes card work on
some paths but excludes device reveals, panel drawing and hidden-buffer posting.
A 14 us render value cannot rule out substantial frame work or scheduling delay.
`render_timing_scope` documents this retained field's scope.

Each missed-refresh record now carries a `timeline`. Its timestamps are microseconds
from one process-monotonic epoch and must not be compared across sessions:

- `previous_observation_us` to `telemetry_observed_us` bounds the counter delta.
  The repeated refresh may precede the current frame. Previous active sequence,
  owned-refresh count and deltas are retained, alongside the current posted sequence
  and active/pending sequences observed when the post was verified.
- `frame_begin_us` precedes callbacks and tooling maintenance. `tooling_tick_us`
  includes window finalisation and metric publication when they run in that tick.
- Render, custom-draw and presenter start/end timestamps expose work omitted from
  `ui_render_us`. Pre-render wait, hidden copy and publication durations are separate.
- `post_request_start_us` and `post_verified_us` bracket the real latch request and
  its status verification. They do not estimate these timestamps from copy durations.
  `latch_request_us` and `post_status_us` retain existing measured component durations.
- `confirmation_wait_start_us` to `active_observed_us` covers the pacing/completion
  wait; completion polling time is separate. These are host observations, not an
  exact FPGA flip timestamp. `telemetry_observed_us` also exposes work between
  completion observation and counter sampling.
- Refresh period, frame-start phase and presenter-start phase remain available.
  These phase ages are host pacer estimates; compare the request boundary with the
  interval and accepted presentation evidence before claiming a late latch post.

The detail records and workload totals freeze with the device-clock window.
`last_dropped_frame_record` in cumulative metrics preserves later evidence without
silently extending that window. Restarting measurement resets the window evidence.
JSON serialisation remains in metric publication; per-drop capture uses fixed-size
values and the pre-reserved detail buffer.

The retained `refresh_hz` field divides observed owned refreshes by the whole
window duration. Idle baseline resets exclude deliberate reuse, so a mixed
activity/idle window cannot use this value as the physical display frequency.
`refresh_hz_scope` identifies that limitation. Use the timeline refresh period
and physical refresh evidence for pacing analysis.

For a complete navigation route, choose a window long enough to cover every return
step and let it finish after motion settles. Keep cumulative step totals distinct
from the frozen window. Older 28-second route measurements can finish during return
animations; their synchronous finalisation may add workload that affects subsequent
cumulative step counts. Preserve those samples and their original contract, but do
not treat them as production-only measurements or invent missing phase timestamps.

Native request durations accept integer milliseconds from 1,000 to 45,000;
missing/null durations retain the five-second default. Unsupported durations
produce a measurement error rather than silently switching to that default.
Completed windows retain `requested_duration_ms` and `target_duration_ms` beside
actual `elapsed_ms`. Profile windows still use their existing ten-second target.

Opt-in frame-neighborhood capture reserves at most 4,096 records before the
window starts. This covers dense drops throughout a 45-second 60 Hz route
without per-frame allocation. Overflow remains explicit and fails complete
evidence validation. Capture OFF reserves no neighborhood storage. The bound
was increased after a CRT 240p route exhausted the original 256 records.

Detailed-capture sessions require `metrics-body-16m-v1`: the service carries
up to 16 MiB of JSON in a metrics body, with a bounded error above that limit.
The host negotiates this capability before starting a session and upgrades a
compatible service through the normal native path when needed. Ordinary
measurements continue to accept the existing metrics capability.

`card_reused_frames` counts confirmed delivery of a requested generation that
was also presented previously. An unchanged quantized pose is valid delivery;
only a delivered generation different from the currently requested generation
is a missing pose. Physical repeat accounting remains separate.

A frame's `motion` describes its pre-render state, including a final animation
endpoint. `motion_continues_after_present` describes whether another moving
frame is expected after that frame was confirmed active. Late endpoint frames
still count; intentional reuse after a confirmed endpoint does not.
`motion_endpoint_resets` and `idle_baseline_resets` expose those boundaries.
The latter requires the renderer's no-work branch and an inactive motion signal.

`phases.direct_hidden_copy_us` is nested within the render interval on the
direct card path. The timeline's `hidden_copy_us` covers later presenter work;
zero there does not imply the earlier direct copy was free.

Detailed phase evidence also exposes `pre_input_boundaries_us`, twelve absolute
process-relative timestamps: callbacks/tooling, timers/feedback, lifecycle,
raw-device poll, readiness, catalog, media, launch lifecycle, navigation,
qualification, benchmark, and view housekeeping. Subtract the preceding boundary
(or `begin_us` for the first) to attribute pre-input wall time. These clocks run
only for phase captures. `destination_stage_us` contains nested wall-time totals
for preview, list, Home, and snapshot preparation. These values are portions of
custom composition, not additional frame cost. `direct_hidden_copy_bytes` records its transferred bytes.
`home_composition_us` splits native Home rasterization from Slint overlay
composition; both are nested within the render interval.

`destination_list_us` splits the nested list stage into row preparation and
composition. HDMI can prepare matching visible rows on CPU0 while its hub is
visible; UI drawing adopts only matching title, badge, favourite and width data,
and never waits for that worker. Its CPU appears in the process/thread report,
while these two fields measure only the remaining UI-thread work.
