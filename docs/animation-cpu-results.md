# Animation CPU campaign results

These hardware results were collected before PR #216's app-wide frame clock.
The branch is now rebased onto `d3dd4522b`; no post-rebase device comparison has
been collected. Late frames now slow motion instead of skipping ahead in time,
so do not treat the numbers below as results for the current branch. Fresh
parent/candidate runs must share the vsync-locked clock and settled-input route
contract documented in `animation-cpu-campaign.md`.

The campaign uses the merged reporting contract and the complete 45-second
Root → Consoles → Nintendo → SNES hub → Games → Root route. Every route below
used a fresh native Dev lease and completed all three repetitions. CPU is
process CPU per confirmed moving presentation, including background app threads;
it is not CPU utilization or isolated renderer CPU. Emulator runs establish
pixel correctness only. The 60 FPS / zero-drop target remains unmet.

## Proven opaque projected compose — kept (`66e082f74`)

The generic kernel checks alpha through NEON-to-ARM transfers inside its loop.
The replacement uses the existing canonical eight-source-row opacity contract
to partition the projected output into generic rounded caps and an opaque
interior. Both bilinear inputs stay inside that interior. Geometry, sampling,
quantisation and presentation timing are unchanged.

| Measurement | Parent | Opaque candidate |
| --- | --- | --- |
| Compose, four native samples (ns/pixel) | 64.780–65.001 | 52.921–53.047 |
| Complete-route drops | 36 / 39 / 36 | 21 / 26 / 23 |
| Mean complete-route drops | 37.00 | 23.33 |
| Moving CPU per presentation (ms), three runs | 20.431 / 20.488 / 20.500 | 19.722 / 19.887 / 19.752 |
| Mean moving CPU per presentation (ms) | 20.473 | 19.787 |

Compose was approximately 18.5% faster. Route drops fell about 37%, and moving
CPU per presentation fell 3.4%. Card-workload drops were 27/29/27 before and
13/18/14 after. Slint and system-transition drops remained: 9/10/9 before and
8/8/9 after. These three-run comparisons describe the measured workload, not a
guarantee under other device load.

Validation: 177 portable scene tests passed (one ignored), focused Clippy passed,
and ARM parity covered 40,000 projected profiles plus exhaustive interpolation,
blend and final-over cases. The native fixture also compared the entire output
before timing. Generic and opaque kernel samples came from the same executable,
with production compiler flags; their order was reversed on the second pair.

Parent Dev SHA256: `9fc4e1c06b67a5e1c75bed032685d05ae775e2f0532584911f061198c85cc19d`.
Candidate Dev SHA256: `730a2c546f3efbcd62d964471810314dbb066d91ba2b20551f78212bfe927217`.
Route evidence: `build/magik-results/20261002T185943Z-ec4a3edada6a` and
`build/magik-results/20261002T190541Z-f41e9f862f91`.
Raw evidence stays ignored; it is not shipped with source.

## Four-column transpose — rejected

A standalone trial composed four independent projected columns, transposed the
four-row NEON results, and stored contiguous four-pixel row runs. It retained the
opaque loop for rounded caps and unmatched tails. Ten thousand randomized ARM
profiles with independently varying source starts/strides matched generic pixels.

On the same native fixture, block compose measured 55.579–55.700 ns/pixel versus
52.922–53.192 for the opaque column loop. Reversing run order confirmed the
regression, approximately 5%. Contiguous stores alone did not pay for the extra
live column state and transpose work in this implementation. No production
integration was made, so another full-route deployment was unnecessary.

Trial source, executable and raw results are retained locally in ignored
`outputs/animation-cpu-round-2/block-*` and `compose-block*`. This rejects the
measured four-column implementation; an eight-column or different layout remains
unmeasured. It does not establish that every row-store approach is slower.

## Flat opaque compose — production change discarded

Four native fixture samples measured 34.805–35.049 ns/pixel for the opacity trial
versus 50.850–50.892 for generic flat compose (about 31% faster). The integrated
trial also passed 4,000 randomized flat-strip cases and the existing ARM parity
suite. Its three complete routes recorded 25/23/19 drops versus 21/26/23 for its
opaque-projected parent. Mean moving CPU was 19.806 versus 19.787 ms/presentation;
mean summed band wall time was 18.788 versus 18.866 ms/card frame. These small
route differences do not establish an improvement.

The fast path was removed instead of retaining production complexity for an
unproven benefit on the target route. The audit also removed flat-only fixture
dispatch and parity cases added for that discarded experiment. Existing
flat-renderer coverage remains. Trial Dev SHA256 was
`466d96ee1e0843e4184373d5834cf39d47a6ce487db71e92374559b28b954ba1`;
route evidence is `build/magik-results/20261002T191612Z-bd8dc35a2a54`.

## Sparse hidden-slot copy — kept (`4078b36aa`)

The native path now copies the carousel and bounded chrome bands during a trick,
with full title coverage for wider breadcrumbs and their clearing. Each slot is
fully seeded when its content generation changes. Chrome updates still run when
the tile image is resident; a partial write makes the slot unknown.

Three native routes passed with 24/26/24 drops. Mean hidden copy time fell from
1.616 to 1.293 ms/card frame (20%). Moving process CPU averaged 19.683 versus
19.787 ms/presentation. That small CPU change and the overlapping drop ranges
do not establish a cadence improvement. The measured copy reduction is real;
60 FPS with zero drops remains unmet.

Validation: a full-pixel alternating-slot test covers both trick directions,
foreign breadcrumbs, interruption and landing. All 42 latch tests passed,
including independently fading chrome and partial-write reseeding. Focused
frontend and scene Clippy passed. Route evidence:
`build/magik-results/20261002T194313Z-bde78ed8112f`; Dev SHA256:
`c9672f1d71e40d4285a07af6b47ec133775a15b5d9f2ea315f21abb5b9eb494a`.

## Filter/shade fusion — rejected

A standalone trial kept the horizontal and mip interpolation floors, then shaded
the result in registers before its final store. Twenty thousand randomized
combinations of lengths, interpolation/mip weights and light factors matched
the separate kernels exactly in ARM emulation. Native timing used 64 fixed
combinations and the production compiler flags, reversing execution order for
confirmation. Separate kernels measured 16.281–16.300 ns/pixel; fusion measured
16.483–16.535, about 1.3% slower. No production integration was made.

Local trial source and evidence remain ignored in
`outputs/animation-cpu-round-2/fused-*` and `filter-*`.
The reflection reordering trial measured a small native kernel gain but remains
unqualified on the complete route. Further campaign items remain in
`animation-cpu-campaign.md`.

## Helper real-time scheduling — removed after audit

The historical trial (`7dac6c9d3`) temporarily gave the helper round-robin
priority 1 while rendering an active trick/carousel frame. The scope restored
its captured calling-thread policy before the helper waited or reported
completion. Production defaults and UI/Main policy stayed unchanged. Initial
wake delay before entering the scope was outside this trial's effect.

One valid route recorded 28 drops before a second route's measurement window
failed to finish. A full repeat passed with 20/23/25 drops and 19.583/19.740/19.642
ms moving CPU per presentation. The parent recorded 24/26/24 drops and averaged
19.683 ms/presentation. The overlapping ranges, small CPU difference and earlier
28-drop sample do not establish a useful whole-route improvement.

Evidence: `build/magik-results/20261002T200633Z-c148ad834ba9` and
`build/magik-results/20261002T201139Z-c6d21a38e2fa`; trial Dev SHA256:
`d584e649cc9b90d31223d3a8bdacdc1fb679ebd92e5453b8dbc032a2fc5c373b`.
The audit removed the scheduler scope, render-worker callbacks and dedicated
policy scenario. Neither whole-route benefit nor native activation was
sufficiently qualified to retain runtime machinery. Future scheduling trials must establish
activation and Main/input behavior before integration. A later diagnostic found
Dev had been replaced by SHA256 `4081d563837504e238668791fa956ef6c4b8022986d68b5050b9a4ad2df928dc`;
its scheduler snapshots are excluded from trial evidence. Reserve Dev for the
remaining matched measurements; do not overwrite another task's executable.

The timeout remains unexplained; the route completed and Main reported zero
crashes/invariants afterward. Timeout handling now preserves the final metrics
for diagnosis. Do not mix incomplete windows into the performance comparison.

## Simplification and deletion audit

Removed the unqualified helper scheduling experiment, its diagnostic-only
scenario/CLI option, and orphaned flat-only benchmark/parity additions. The
renderer uses its original constructor and thread policy;
no per-frame scheduler callbacks or temporary policy guard remain. Retained the
measured opaque-compose and sparse-copy changes, their pixel/coherence tests,
moving-CPU accounting, bounded metrics transport and timeout evidence capture.
The audit changes no artwork, poses, durations or input behavior. New hardware
measurements require an isolated Dev build; previous numbers remain historical.

Audit validation: 42 latch tests, two renderer coordination tests, one full-pixel
sparse-copy test and two host scenario-selection tests passed. Frontend/scene
Clippy, Ruff and bounded Rust LSP diagnostics passed. ARM compilation and exact
NEON parity passed in local emulation; the retained benchmark also passed its
output check. Emulation timing is excluded from performance evidence. The audit
performed no MiSTer deployment or device control.

Post-rebase validation: 24 card-session, 42 latch, 14 tooling, four frame-clock,
seven trick/pixel, five host-wait/selection tests and the animation-time source
check passed (97 tests). Frontend Clippy passed and the Cortex-A9
`release-device-ui-tests` application built successfully. No device deployment
or post-rebase performance measurement was performed.


## Fresh frame-clock profile and clean baseline (`8fe628a72`)

Dev was explicitly reserved for this session. The complete instrumented SNES
round trip passed with a 45.005-second window, 378 card frames, 378 collected
helper frames and matching `vsync-locked-v1` context (16,666,667 ns/period).
The sampled executable SHA256 was
`c0b542563734611d2b88984b994c007ed7589e9b273c0d52768b5d61d2fc879b`.
Evidence: `build/magik-results/20261002T220830Z-93e66a7f17c6`.

| Stage | Total wall time (ms) | ms per parallel card frame |
| --- | ---: | ---: |
| Compose | 2703.71 | 7.153 |
| Geometry/filter | 2367.43 | 6.263 |
| Reflection draw | 828.32 | 2.191 |
| Reflection preparation | 525.97 | 1.391 |
| Clear | 317.71 | 0.840 |
| Helper merge | 265.60 | 0.703 |
| Hidden-slot copy | 493.18 | 1.305 |

Renderer stages sum both threads' wall time; they are neither critical-path
elapsed time nor stage CPU time. Slint raster contributed 182.90 ms across 27
calls (6.774 ms/call, maximum 15.338 ms). The CPU sampler collected 1,313 stacks;
compose and filtering lead the sampled renderer leaves. Profiling increased
cost: its 43 drops are diagnostic evidence, not an acceptance result. An earlier
10-second profiled window was rejected and excluded from whole-route attribution.

Three clean routes of the same executable passed. Evidence:
`build/magik-results/20261002T221025Z-891d1ca3da50`.

| Repeat | Drops | Process CPU / moving presentation (ms) | Hidden copy / card (ms) | Producer p99 (ms) | Card / Slint / system drops |
| --- | ---: | ---: | ---: | ---: | --- |
| 0 | 28 | 19.9417 | 1.3146 | 14.457 | 20 / 1 / 7 |
| 1 | 26 | 19.6447 | 1.2721 | 15.335 | 19 / 1 / 6 |
| 2 | 25 | 19.9250 | 1.2968 | 15.799 | 15 / 1 / 9 |

Mean drops: 26.33. Mean process CPU: 19.8371 ms/moving presentation. This is the
current sparse-copy baseline after the app-wide clock change; earlier route
numbers cannot establish a current improvement. Slint/system misses remain
outside card composition and require separate pacing analysis.

## Helper merge elimination (#8) — kept (`6f9f89475`)

Native publication now takes the primary and helper buffers as separate immutable
tile sources, using the actual rendered split before adaptive balancing chooses
the next one. This removes the intermediate helper-to-primary copy without
sharing mutable framebuffer storage or allocating another frame. Full-frame
consumers merge a retained helper on demand, including after direct publication.
The seeded hidden slot receives both current bands before publication.

Serial pixel parity covers changing splits, ordinary poses and both directions
of root/nested tricks through landing. Card-session, retained-latch, motion-clock
and full-frame fallback checks pass; frontend tooling Clippy passes. Compare
three clean candidate routes against the baseline above. Rendering, animation
timing and input behavior are unchanged.


The clean candidate check passed all three routes and teardown. Executable SHA256:
`7be5c9a060c45d64d21a37214f3da819b845962d5a7c2b05b348e6998c9491a5`.
Deployment: `build/magik-results/20261002T223644Z-01fef60554be`.
Route evidence: `build/magik-results/20261002T223829Z-85f17e8e5da0`.

| Repeat | Drops | Process CPU / moving presentation (ms) | Hidden copy / card (ms) | Producer p99 (ms) | Card / Slint / system drops |
| --- | ---: | ---: | ---: | ---: | --- |
| 0 | 23 | 19.2965 | 1.2781 | 13.336 | 13 / 1 / 9 |
| 1 | 23 | 19.2938 | 1.2693 | 14.740 | 13 / 1 / 9 |
| 2 | 20 | 19.2770 | 1.2662 | 13.504 | 11 / 1 / 8 |

Mean moving CPU fell from 19.8372 to 19.2891 ms/presentation: 0.5481 ms, or 2.76%.
Every route had 521 moving presentations. Mean producer time fell from 10.9370
to 10.2066 ms/card (0.7304 ms). Mean merge time fell from 0.7149 ms to zero.
Summed band wall time was effectively unchanged (19.0999 to 19.0445 ms/card),
consistent with removing the intermediate copy rather than changing raster work.
The clean route consistently saved CPU; retain the change on that evidence.

Drops fell from 28/26/25 to 23/23/20 (mean 26.33 to 22.00, 16.5%). Card-attributed
records fell from 20/19/15 to 13/13/11. Three repetitions do not establish a
precise cadence effect or zero-drop qualification. System transitions still
recorded 9/9/8 misses and Slint one per route. Workload attribution describes
activity at observation, not proven cause. Latch rejections were zero; posted
and flipped counts matched in every route. Full-frame fallback copies still
occurred and the route reached all expected endpoints.

Next experiments remain one-frame-ahead tricks (#14), then a fresh scheduler
comparison against sparse copy with explicit policy activation evidence. Neither
has a post-frame-clock verdict. Buffering must preserve every virtual-clock
step and visible landing/input ordering, including preparation holds; it must
not conceal insufficient sustained throughput. The old sparse-copy A/B remains
historical; the new clean baseline establishes its current cost but does not
isolate its causal saving under the new clock.


## Device-reveal attribution after merge removal (`817f57bd9`)

The complete instrumented route passed with a 45.002-second window, 378 card
frames and 118 HDMI reveal frames. Evidence:
`build/magik-results/20261002T224716Z-29293c641aef`; executable SHA256
`ac06bc258322727d8ebf13229bbc8b056bc741b41e42e085513649593a71e74b`.

| Reveal stage | Total wall time (ms) | Mean per reveal frame (ms) | Maximum (ms) |
| --- | ---: | ---: | ---: |
| Background fade | 293.354 | 2.486 | 9.243 |
| Card-face sampling | 74.516 | 0.631 | 3.830 |
| Device sampling | 509.503 | 4.318 | 6.903 |
| Outline | 13.055 | 0.111 | 0.382 |
| Page bands | 140.838 | 1.194 | 4.162 |

These are diagnostic wall times, including scheduling and instrumentation.
They identify device sampling and the scalar full-screen fade as useful next
CPU targets. Slint raster contributed 188.044 ms across 27 calls.

The clean merge-candidate record for sequence 529 (repeat 1) reports 14 us
`ui_render_us`, but custom drawing spans 14,494 us before the hidden copy and
post. That counter explicitly excludes custom drawing. This is a borderline
full-frame deadline, not evidence by itself of a pure pacing/latch bug. Other
records also show expensive transition starts and endpoint Slint raster; these
remain in scope and must not be hidden by narrowing the drop gate.

## Exact reveal background fade — kept (`32e19d82f`)

HDMI background fade now reuses the existing RGB565 black-blend kernel with the
same five-bit alpha buckets as scalar `card_page::blend`. Zero/full opacity uses
fill/copy. No new kernel, allocation, alpha rounding, artwork, animation step or
duration is introduced. Scalar and ARM output agree for all 65,536 RGB565 colours
and all 257 input alpha values. Existing reveal tests and focused Clippy pass.


All three clean fade-only routes passed with zero latch rejections. Evidence:
`build/magik-results/20261002T225257Z-1b9aadc53226`; executable SHA256
`f829db83fba8ee937c8e85add34f5aa378458f041b149de9ad7dd1f0cebe62d9`.
Drops were 22/21/20, with CPU 19.0023/18.9740/19.0313 ms/moving presentation.
Mean CPU fell 19.2891 -> 19.0025 ms (1.49%) versus its merge-only parent.
The one-drop mean difference (22 -> 21) does not establish a cadence improvement.

## Exact device sampling clip (#11) — kept (`489a79f91`)

Device sampling intersects the rounded card span with the valid source-column
interval and rejects invalid source rows before the pixel loop. The interval
comes from the existing floor-mapped coordinates; no independently rounded
floating-point bounds or approximation replaces the projection. Positive scale
makes those columns monotonic. Transparent pixels and opaque black screen pixels
keep their original behavior. There is no new storage or unsafe code.

Forty-eight full-frame hashes captured before clipping match afterward for four
card positions and twelve fade/scaling boundary times, including hub and list
pages. All seven reveal tests also pass on the ARM backend, including this
matrix; exhaustive black-fade parity and NEON offset/tail/alpha checks passed.
Focused Clippy and semantic diagnostics are clean.

The three clean clip-candidate routes passed with zero latch rejections.
Evidence: `build/magik-results/20261002T225843Z-08d56ff3a48a`.
Executable SHA256:
`dd10a45529be4e5a7535f8f0895d683a478caa87c0682f1b5af32a77a519626e`.

| Repeat | Drops | CPU / moving presentation (ms) | Card / Slint / system drops |
| --- | ---: | ---: | --- |
| 0 | 21 | 19.0729 | 12 / 1 / 8 |
| 1 | 19 | 18.6820 | 13 / 1 / 5 |
| 2 | 24 | 18.7062 | 15 / 1 / 8 |

Mean CPU is 18.8203 ms, 0.1822 ms (0.96%) below its fade-only parent. Card CPU
averages remain close: the clip changes reveal work, not card raster. The first
repeat is slower than the parent samples; do not treat a sub-percent route
average alone as decisive evidence. Drops are effectively neutral: 21/19/24
versus 22/21/20. Producer tails vary with helper run delay and remain unresolved.

A second complete diagnostic route passed with the same 118 reveal frames and
378 card frames. Evidence: `build/magik-results/20261002T230617Z-37726dfb8867`.
Its 45.010-second window confirms substantial savings in the intended stages:

| Reveal stage | Before both changes (ms/frame) | After (ms/frame) |
| --- | ---: | ---: |
| Background fade | 2.486 | 1.288 |
| Device sampling | 4.318 | 3.179 |
| Card-face sampling | 0.631 | 0.623 |
| Outline | 0.111 | 0.107 |
| Page bands | 1.194 | 1.188 |

Keep the clip on exact pixel parity, directly attributed sampling savings and
the consistent direction of route CPU totals, with no cadence claim. Diagnostic
wall times are not stage CPU estimates, and their drops are not acceptance runs.

The goal remains perfect moving-refresh cadence throughout the launcher and
subviews. No animation steps, durations, source artwork or drop gates were
changed. Roughly 21 drops per route remain. The next work must address card tail
scheduling/render-ahead and cold destination/endpoint Slint preparation; cheaper
steady-state reveal drawing does not resolve those first-frame costs.
