# Mini workloads

The benchmark command runs an installed Mini workload directly. It does not
start a Slint session or wait for framebuffer readiness.

```
scripts/magik bench blend
scripts/magik bench blend --visual
scripts/magik bench blend --counters neon
scripts/magik bench blend --counters memory
```

Bare `bench` means `bench blend`. Timing runs exactly twice. Each explicit PMU
command runs once. Visual mode runs one pass and resumes Main automatically;
it cannot be combined with counters. No command automatically retries a workload.

The host reuses compatible services and installed artifact hashes. Cargo decides
whether source builds need compilation. It records build, upload, execution, and total durations in the result
bundle. A service update requires the `run-benchmark-v2` capability, not a matching
build identifier. Adding a workload to Mini does not require a service update.

The native service launches only its installed Mini executable with
`--bench WORKLOAD --mode MODE`. Workload names begin with a lowercase letter,
contain lowercase letters, digits or hyphens, and are at most 48 bytes. Mini owns
recognition and reports unknown workloads. There is no shell or arbitrary path.
Modes are `timing`, `visual`, `pmu-neon`, and `pmu-memory`.

Execution and pipe draining share a 30-second deadline, or 90 seconds for visual
mode. Client disconnect, timeout, and output overflow terminate the process
group. stdout is bounded to 256 KiB and stderr to 8 KiB; cleanup has a bounded
reap interval. Main restoration is attempted on every execution outcome. Host
request deadlines additionally allow Main handoff/restoration. Uncertain transport
outcomes are reported without rerunning the workload.

## Results

`benchmark-raw.json` preserves received bytes, including invalid/truncated output.
`benchmark-process.json` records exit status, artifact identity, stderr, execution
errors, and Main restoration. Validated `benchmark.json` adds host provenance.
Build/upload phase evidence and source dirty status are in `run.json`. Artifact
SHA-256, Git provenance and target/build flags identify new results; the former
custom source fingerprint is no longer produced or required for comparisons.
Historical results containing that extra field remain readable.

Schema version 1 contains workload, mode, artifact SHA-256, correctness, fixture
identity and structured metadata, positive work count, samples, and optional
visual summary. Each sample identifies its repetition, fixture and work count,
and includes elapsed nanoseconds and normalized time. Visual summaries contain
presentation statistics, never kernel timing samples. PMU records remain inside
their single diagnostic sample and are prohibited in unprofiled results.

The host validates identity, sample counts, normalized timings and presentation
success. PMU validation requires complete counters and equal nonzero enabled and
running times. Missing counters are errors, not zeroes. Measured zero refills are
valid. NEON clock-enabled cycles do not measure utilization; L1 refills are not
DRAM misses. Event 0x61 is data-cache-dependent stalls despite its legacy key.

`magik.benchmark_compare.compare` compares two loaded result objects offline.
It requires matching workload, mode, fixture, work count, target/build flags and
device, with clean source provenance. It never launches a process. Two timing
samples are a practical comparison, not statistical proof. PMU comparison is
one diagnostic pass per version; environment differences are reported.

## Adding a workload

Implement Mini's preparation, correctness check, and execution; register its name
in Mini's dispatch and return the existing result contract. Reuse the command,
transport and result validator. Keep input preparation outside timing, perform
correctness first, and make profiling explicit. Do not add host/service dispatch
branches, UI controls, automatic matrices, or a campaign engine.

A focused local oracle test and one bounded invocation should establish a new
workload. Stop and discuss inconsistent or slow results instead of tuning in a loop.

`home-count-refresh` measures one root and one nested count refresh in each
of its two timing samples. It uses the production portable card renderer at
960x540 with six generic cards and changes only the first count. Cache seeding
and an exact cold-render pixel comparison run outside timing. Preparation,
resting raster generation and disposal run inside timing. It does not measure
physical input or UI-thread latency; validate those with the real app journey.

`retained-home-tiles` publishes the same pair of immutable Home tiles eight
times, alternating qualified hidden slots. Each timing sample excludes chrome
seeding, records tile bytes and copy time, and includes a complete physical-slot
pixel oracle and eight protocol-v5 posts/confirmed flips. Wall time includes
readback and latch waits; copy time is recorded separately. Repeated-vblank
deltas remain separate from copied bytes. Main resumes after the invocation.

`catalog-sort` calls the shared production ASCII title sorter on fixed-seed
1,000, 10,000 and 50,000-row fixtures. It checks stable ties and non-ASCII
semantics against the original comparator and compares complete rows outside
timing. Separate observation passes report normalization calls, allocation
counts, allocated bytes and peak temporary bytes. Input construction, clones,
and exact-output comparisons are outside the two timed sorting samples.
The same Mini System allocator adapter runs on both revisions; accounting is
disabled during timing. Cached keys trade bounded temporary storage for fewer
allocations; this workload does not measure complete catalog rebuild time.

`preview-shards` runs the actual incremental source builder for a SNES-only
and Saturn-only request, and the actual bulk builder for both systems. An
isolated development SD fixture contains 10,000 metadata items per shard and
one playable row per system. Setup and fixture removal are outside timing;
profile discovery, metadata loading, decoding, title indexing and enrichment
are included. Exact family asset keys and launch plans are required in every
case. The same per-thread, test-only software decode counter is enabled on both
revisions. Catalog diagnostics go to a fixture file to preserve the bounded
native result pipe. These are warm filesystem samples, not cold-media latency.

`incremental-refresh` mutates one loose file in each sample and runs the actual
incremental planner, rebuild and publication against an isolated development
SD source/catalog fixture. Source setup and the initial cold publication are
outside timing. Distinct publisher-directory timestamps are set outside timing
to exercise a changed system independently of SD timestamp granularity. Watch
state is compared with an independent full-tree capture; published artifacts
are compared with a fresh source snapshot outside timing. Counters observe
logical source-walk calls and fallback watch-tree calls, rather than syscalls.
Linux's complete fd observations can be reused. Streaming WalkDir, uncertain
entries, pruned directories or exceeded 65,536-entry / 16 MiB observed-path
limits retain the existing full watch capture. Those limits bound observation
capture; they are not an exact total allocation budget.

## Mapped capsule launch lookup

`scripts/magik bench catalog-launch` measures the production catalog's lookup of
structured launch plans through known view ordinals. Its fixed 1,000-, 2,000- and
4,000-row navigation packs live in owned temporary directories and are removed
on exit. Mapping, hot-row hydration and exact plan checks run before timing;
two unprofiled repetitions report each size separately. The native benchmark
restores the Dev launcher and never launches a core or modifies the installed
catalog. This measures the capsule's second-pass lookup mechanism; full capsule
encoding, cold metadata handling and Main handoff are outside its timing window.

## Carousel motion

`scripts/magik check motion` keeps three five-second tap/reversal windows and
adds one eight-second uninterrupted right hold. Each window excludes two seconds
of warmup. The hold waits for the preceding card spring before pressing, sends
one press and one release, and requires continuous navigation for every measured
presentation. Release is guaranteed when the measurement fails.

Run only that case with `scripts/magik check motion-held`. Add `--profile` for a
separate ten-second sampled diagnostic. `--installed-sha256 HASH` skips building
and delivery when that exact Dev application is already running.

`check motion` first runs a 12.5-second window that plays six right taps 250 ms
apart, holds right for ten seconds and keeps one second for the release to
settle. The device times every press from the window start through the same
input router, so host RPC jitter cannot move them, and keeps the launcher awake
until the window completes; release is still guaranteed on failure. It uses its
own native 60-second test lease, which the shared motion session cannot spare.
Run it alone with `scripts/magik check motion-taps-then-hold` (uninstrumented).

`scripts/magik check screensaver` starts the screensaver at once for one
ten-second window, requires every measured presentation to be a screensaver
frame, then wakes the launcher. The user's screensaver setting and delay apply
again afterwards.

Compare Mini's original storyboard with its sustained default-speed workload:

```sh
scripts/magik check concept --app mini-magik --concept launcher-cards --production-build --quick
scripts/magik check concept --app mini-magik --concept launcher-cards-held --production-build --quick
```

The held Mini case measures eight seconds of continuous seven-card-per-second
motion, feeding position directly to the shared renderer without pausing or
restarting a spring at each card boundary. Its quick check retains focused
captures after measurement. The original storyboard remains available. These
fixtures isolate rendering; they do not exercise production input or nested
menu transitions.

Production motion windows report `dropped_frames`: every missing fresh animation
frame, including a repeated moving carousel generation. Settled idle frames are
excluded. Confirmed display activations and generation accounting cover distinct
refreshes; software target observations are not added again. Bounded records
include worker execution and delivery costs, with a specific budget-overrun cause
when measured and an explicit unknown cause otherwise. CPU sampling is disabled;
thread CPU clocks remain available. A run measures performance and does not claim
zero drops merely because its scenario checks passed.

## Full launcher navigation campaign

`scripts/magik check animation-app` runs three separate windows for each route:
all root cards in both directions; Consoles → Nintendo → SNES → Games;
Computers → Sinclair → ZX Spectrum → Games; Handhelds → Nintendo → Game Boy
→ Games; Arcade hub/list/drawers; global Favourites; and the Settings menu.
Each route returns to the root. Settings only opens its menu and changes focus;
Favourites opens its list and returns. Games are never launched.
This covers each category and representative hierarchy depths, not every system.
Use `check motion-held` separately for uninterrupted carousel rotation.

Repeat `animation-app --frame-evidence phases` for bounded drop neighborhoods
with per-phase CPU clocks and helper timestamps. Run `animation-app --profile`
separately for one sampled profile per route. Set `MAGIK_ANIMATION_ROUTES` to a
comma-separated subset of route names for focused diagnosis.
`MAGIK_ANIMATION_REPETITIONS` accepts 1–10 ordinary or frame-evidence windows
per route (default three); sampled profiles remain one per route. Every completed
sampled profile is retained under `profiles/RUN_ID/`; root-level profile files
remain the last completed profile for compatibility. Events link profiles to routes.
Each profile directory also contains `profile-quality.json`, validating sample
totals and reporting thread/symbol coverage, symbol depths and attribution limits.
Completed raw windows are saved before validation, including when a check fails.

Compare ordinary and diagnostic runs on the same executable hash and record the
selected display mode and authoritative scanout geometry. CRT uses the shared
compositor path; helper-only checks apply only when `card_helper_ahead` reports
`native-tricks-v1` or `native-browse-tricks-v2`. The latter also prepares the
next ordinary browse pose from the existing FrameClock prediction; generation
and source matching still reject changed input. On CRT, use process CPU and
whole-frame transfer evidence. A passing scenario proves route execution and evidence consistency, not zero dropped frames.
CPU samples are aggregated over a route; thread sample shares are not calibrated
CPU shares (use measured thread CPU clocks). Samples do not identify the stack at an
individual missed deadline. Frame neighborhoods retain host timing and CPU evidence,
but FPGA acceptance/cutoff times are unavailable. Keep that attribution limit explicit.

Pin a campaign with `MAGIK_EXPECT_DISPLAY_MODE=crt-240p60` and
`MAGIK_EXPECT_RENDER_SIZE=640x240`. The fixture verifies Main's mode before
and after every session; route, motion and idle measurements reject wrong
geometry, including stale embedded windows. Hub controls follow the app’s reported runtime axis, including CRT and HDMI
portrait without a display pin. These guards do not change mode.
Arrange exclusive device use across concurrent chats before a long campaign.
