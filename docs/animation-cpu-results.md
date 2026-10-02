# Animation CPU campaign results

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
