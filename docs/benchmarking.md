# Benchmarking And Profiling

This document defines current benchmark policy. Dated measurement logs live in
`history/`; use them as evidence, not as the command surface.

## General Rules

- Use RGB565 for production launcher and arcade conclusions. The UI/app
  benchmark path is RGB565-only; wider-color env overrides and color-route
  smoke paths are deleted from the app.
- For each performance-changing commit, run the targeted "before" benchmark on
  the previous implementation, make one logical change, then rerun the same
  command shape after validation. Commit messages and evidence notes should name
  the labels and the metric that changed.
- Prefer short targeted benchmarks over broad soak runs. Scroll-path changes use
  one 30s `turbo-hold` run unless the code path needs a different scenario.
  Launcher pacing and frame-timing refactors use one 30s `human-turbo-hold` run
  because it exercises human-like bursts, pauses, and reversals while preserving
  the real Arcade entry flow.
  Avoid duplicate long scroll and turbo-scroll runs for the same claim.
- "Still 60fps" is not evidence by itself. Report the metric owned by the
  change: for example preview decode latency, catalog apply time/backlog,
  launch-prep p50/p95, `arcade_list_update_us`, or framebuffer present p95/p99.
- Before committing a performance change, run code review against the exact diff
  and tidy any findings before the final validation/benchmark rerun.
- Start visual benchmarks from a clean display-owner state. If stock OSD/menu is
  visible over the benchmark, the run is invalid even if the framebuffer PNG
  looks correct.
- Beware contaminated 30fps/vsync cadence after repeated restarts or immediate
  post-deploy runs. Settle, reboot, or rerun before declaring regressions.
- Do not compare short runs whose first seconds show `fps ~ 30` unless that is
  the behavior under test.
- Arcade benchmarks must run through `MiSTer_MagiK` supervising
  `mister-magik-fb ui launcher 0`. The removed direct Arcade scene is invalid
  for current performance conclusions because it bypasses Main's OSD, VT, and
  input ownership setup.

## Binary Scope Labels

Benchmark `run_context_tsv` rows identify the binary artifact that was expected
for the run:

- `binary_scope=prod-all`: production `release-device` build with
  `ui_scope=all`, suitable for whole-app release checks and general scene
  benchmarks.
- `binary_scope=launcher-scope`: production `release-device` build with
  `ui_scope=launcher`, suitable for launcher/Arcade production scroll evidence.
- `binary_scope=fast-launcher-scope`: optimized thin-LTO `release` build with
  `ui_scope=launcher`. It is daily-deploy evidence rather than production
  release evidence and records `production_restore_required=1`.
- `binary_scope=profile-launcher-scope`: profiling `release-device-profile`
  build with `ui_scope=launcher` and `features=ui,profile`, suitable only for
  CPU profile artifacts.
- `binary_scope=deployed-unknown`: `--skip-build` run where the script did not
  deploy or fingerprint the binary currently on the MiSTer. The row still
  records the expected local `profile`, `features`, path, and size, but runtime
  comparisons must account for possible stale profiling or alternate binaries.

Production acceptance runners do not permit `deployed-unknown`.
`profile-preview-scroll.sh`, `device-catalog-acceptance.sh`, and
`profile-first-scan.sh` require the deployed
SHA-256 to match the local binary and the local `.features` sidecar to
match the declared feature set. First scan verifies the embedded frontend;
standalone builder profiling has its own explicit harness contract. ARM builds also emit a hash-bound
`.build-receipt.tsv` beside each binary; it records the build-time source
commit, dirty state, profile, feature set, and UI scope. Benchmark contracts
reject a missing, stale, or mismatched receipt. Context rows separately record
the checkout state at run time, so a dirty run remains reproducible evidence
only when its diff is retained with the artifacts.

Do not compare these as if they were the same artifact. A CPU-profile run must
be read as profiling evidence, not production frame-time evidence, and the
production `release-device` binary should be redeployed after any profiling
binary has been installed on the MiSTer.

## First Scan Gate

The cold first-scan gate measures time to the first-visible Arcade projection
and time to a durable V3 registry. Its default preserves the locally generated
Arcade bootstrap index while removing Catalog V3, which measures the normal
recovery/update path:

```bash
scripts/profile-first-scan.sh LABEL --skip-build --replace-label --thread-sample
```

Use `--drop-arcade-bootstrap-index` for a separate genuine first-ever run. That
mode removes both Catalog V3 and the retained index, so it must keep the safe
foreground MRA scan within its own baseline rather than being compared as an
index-hit regression. Retained evidence must include
`builder_first_visible_ready`, `launcher_first_frame_presented`, and the Arcade
bootstrap probe/publish timing rows.

Pre-V3 historical performance budgets are retained only as comparison data:

- `library_ready <= 96592ms`
- `library_db_saved <= 117766ms`
- legacy persisted bytes `<= 13151232`

The default command records these without failing on them. Pass
`--enforce-performance-budgets` for an intentional performance gate. Catalog
correctness instead uses `catalog-v3-inspect`: registry totals, system count,
Arcade resident rows, state binding, scanner cache, and every system shard must
agree. Resident Arcade rows are never interpreted as the full catalog.

During first database creation, the catalog builder owns the machine. The
catalog worker and library walker must run foreground, with nice `0` and
unrestricted CPU affinity, until the RAM catalog is ready. Active screenshot
pack downloads also run at nice `0` with unrestricted CPU affinity because the
streaming thread, `curl`, and SHA-256 verifier directly drive a visible progress
bar. Do not apply the background CPU0/nice policy to these first-build scan or
visible media-download paths. Dropped frames or less-smooth scan-screen
animation are acceptable during this window because the launcher has no usable
catalog yet; failing the readiness gate is not.

The low-priority CPU0 policy remains appropriate for warm validation, preview
prefetch, media coordination, index sidecar repair, and other background jobs
after a usable catalog exists.
Use `--thread-sample` when changing catalog scheduling so the run proves the
first-build roles are foreground and the later background roles remain isolated.
For first-scan runs the sampler targets `mister-magik-fb`, which owns the
embedded builder. Its retained summary
must include builder thread/core residency and `vmhwm_kb`; a header-only sample
invalidates the run.
The post-scan preparation overlap emits
`builder_catalog_prepare_overlap` with `wall_us`, separate audit/stamp/catalog
durations, `overlapped_us`, and the worker policy. Retained evidence must also
contain a `catalog-audit` thread-policy row with nice `0` and all online CPUs;
the optimization is invalid if either branch inherits the background CPU0
policy. Compare `wall_us` with the sequential audit+stamp+catalog sum, but keep
the canonical `library_ready` marker as the reference regression gate.

## Catalog V3 Rebuild And UI-Contention Gates

Catalog V3 has two separate acceptance claims. Neither may be inferred from the
other.

The standalone reconciliation benchmark compares an all-system publication
with a one-system delta using the production schema-one shard writer and
generation publisher:

```bash
scripts/bench-catalog-rebuild.sh LABEL
```

The report also owns the catalog layout metrics for storage experiments:
logical and allocated bytes, file/directory counts, and navigation-open
p50/p95/p99. Compare matched command shapes and medians; filesystem allocation
is meaningful only when the baseline and candidate use the same filesystem.

It runs without the rest of MiSTer MagiK and reports measured elapsed speedup,
full and delta system counts, and exact rebuilt-system work ratio. The default
is 30 systems with 200 games each. Ten times remains a comparison target, not a
release blocker. Correct changed-system selection and a work ratio greater than
one are structural gates; activation evidence must also show the real
changed-system plan and artifacts.
The lab command refuses non-empty storage rather than risking an existing
catalog.

Once any catalog is usable, a build or rebuild is background work and may not
drop a single UI frame. The extended contention gate runs the production
`human-turbo-hold` scenario for at least 120 seconds while forcing catalog work:

```bash
scripts/profile-catalog-contention.sh LABEL --skip-build
```

It retains the normal frame-pacing and latch-drop gates from
`profile-arcade-scroll.sh`, then correlates the frame trace with the
per-thread sample. The harness first requires one exact selected preview, then
freezes further selected-preview jobs so an independent image decode cannot be
misattributed to catalog contention; Arcade list motion continues for the full
run. The boot-entry gate proves that initial exact preview; the generic
all-scroll preview-exact gate is intentionally skipped after previews freeze.
It also skips the search-overlap gate because the rebuild's heavy stages are
supposed to remain paused for the continuous-input window; search readiness is
tested separately. A pass requires at least ten CPU-active catalog sample
intervals and 600 overlapping frames. During those overlapping frames it
requires zero over-budget work frames, zero two-frame wall stalls, only
`vsync` sources, zero vsync miss streak, and successful Main presentation. A
quiet or already-finished catalog run is invalid evidence. Use
`scripts/profile-catalog-contention.sh --self-test` for the parser contract.

## Startup Reveal Gate

Startup reveal checks cover the three launcher entry modes: cold no-catalog,
warm valid-catalog, and return-from-game. Run the acceptance script against a
freshly deployed production binary when changing launcher lifecycle, catalog
startup, preview readiness, or launch-return behavior:

```bash
scripts/device-startup-reveal-acceptance.sh LABEL
```

Set `MISTER_STARTUP_REVEAL_MODE=cold|warm|return` to run one lifecycle in
isolation. The harness restores cold-test catalog backups and clears temporary
launcher handoff state from its exit trap, including interrupted runs.

For the broader hardware-in-the-loop (HIL) release policy, tier semantics, skip
reporting, and artifact contract, see `docs/production-readiness.md`.

The script backs up and removes the device catalog for the cold scenario, then
restores it before the warm and return scenarios. It appends
`history/toolchain-bench/results-startup-reveal.tsv` rows with `mode`,
`reveal_ms`, `input_enabled_ms`, `catalog_ready_ms`, `first_frame_ms`,
`preview_state`, and `result`. Generated TSV rows are measurement evidence and
should not be committed unless a release note or investigation explicitly needs
the captured device run.

Acceptance depends on `/tmp/mister-magik/status.json` reporting
`startup_mode`, `startup_reveal_state`, `revealed`, `input_enabled`,
`reveal_ms`, and `input_enabled_ms`, plus startup timing rows in
`/tmp/mister-magik/events.jsonl` and the launcher log. Warm boots must not emit
`startup_splash_visible`; return-from-game must restore Arcade selection before
`launcher_revealed` and wait for `return_preview_ready`.

## Latch Benchmark Split

Latch-specific launch and Arcade benchmarks prove readiness before collecting
frames. The shared gate requires the manifest-owned RBF in Main's cmdline, the
scanout-slots module and device node, and supported `0x57` and `0x58`
acknowledgements. A valid installed platform may be reactivated through Main;
an invalid platform or unsupported latch aborts the run. Never interpret an
analyzer's later backend rejection as an acceptable benchmark sample.

Use one focused latch benchmark per claim. Do not bundle Home, Arcade, preview,
and copy-path conclusions into one broad run.

- Home render or horizontal pan claims: use
  `scripts/gate-launcher-home-max-scroll-zero-drops.sh LABEL --secs N`.
  Report latch-visible metrics from the generated `*-launcher-home-scroll-drops.tsv`:
  latch deadline misses, visual latch misses, FPGA `drop_count`, latch margin,
  and the specific Home timing under test. The root gate proves focus movement
  plus repaint; a named submenu gate additionally proves real horizontal pan.
- Hidden-copy claims use the Home latch gate and its `latch_copy_p50/p95/p99`
  measurements. The old plugin mapping microbench is retired.
  Report `latch_copy_p50/p95/p99` or the microbench copy timing, not Arcade
  list timing.
- Arcade list or preview claims: use
  `scripts/profile-arcade-scroll.sh LABEL --secs N --scenario turbo-hold` for
  synthetic maximum velocity or `--scenario human-turbo-hold` for pacing
  refactors that need human-like bursts and pauses. Report Arcade/preview
  metrics from the trace plus passive latch evidence from
  `*-arcade-latch-drops.tsv` and `*-fpga-latch-{before,after}.log`.
- Frame-tail/status-write claims: use Arcade turbo when the suspected work
  appears after latch post during active Arcade frames. Report
  `status_write_due_frame_finish_max`, `status_write_deferred_frames`,
  `frame_tail_slack_*`, `work_gt_16667`, fallback/timeout/error counts, and
  latch status.

Performance-impact commits should name the one focused command used for
before/after and the metric it owns. Benchmark/correctness commits may update
the reporting surface without claiming a faster renderer.

## Launcher Menu Row Scenarios

Use `home-repeat-hold` when measuring the experience of holding left or right
on a launcher hierarchy row. The scenario feeds held d-pad input through the normal
launcher input path, so it includes the real motion behavior: an immediate
single-tap move using the reusable critically damped smooth spring, a
200ms hold threshold, acceleration into frame-delta-driven motion at 1440px/s,
then velocity-preserving spring settling onto a directional tile boundary after
release. At either end of the menu it reverses direction, which keeps
long traces exercising both left and right movement.

```bash
scripts/bench-toolchain.sh LABEL --replace-label --device --scene-secs 30 --launcher-scenario home-repeat-hold --ui-scope launcher
```

Use the strict zero-drop gate when the symptom is visible missed frames while
holding left/right across a launcher menu row. Run both the four-item root and
a longer submenu for hierarchy changes:

```bash
scripts/gate-launcher-home-max-scroll-zero-drops.sh LABEL --secs 30 --skip-build
scripts/gate-launcher-home-max-scroll-zero-drops.sh LABEL-CONSOLES --secs 30 --menu consoles --skip-build
```

The gate sets the benchmark-only `MISTER_LAUNCHER_START_MENU` selector. Accepted
values are `consoles`, `handhelds`, `computers`, and `snk-neogeo`; normal
production launcher starts ignore this variable.

The default gate follows the production renderer,
`fpga-vblank-latch-hidden`, and collects passive `fpga-latch-report` samples
before and after the run. Home visual and performance acceptance must use this
latch backend. The legacy `fb0-dirty` path is recovery-only and may tear during
horizontal motion; do not use it for visual conclusions. It can be forced only
for an explicitly labeled fallback diagnostic comparison:

```bash
scripts/gate-launcher-home-max-scroll-zero-drops.sh LABEL --secs 30 --skip-build --present-backend fb0-dirty
```

The gate writes `build/launcher-home-scroll-profiles/*-launcher-home-scroll.tsv`
and a matching `*-launcher-home-scroll-drops.tsv` report. The `/dev/fb0`
fallback gate treats `wall_us > 16667` or `loop_delta_us > 16667` as visual
cadence failure because userspace copies to the scanned framebuffer after
vblank. The FPGA latch gate uses latch-visible evidence instead: every measured
frame must use the latch backend with status `ok`, post before the latch deadline,
alternate hidden buffers, keep sampled FPGA flip counters consistent when they
are present, and finish with passive `fpga-latch-report drop_count=0`.
`wall_us` and `loop_delta_us` remain in the latch report as
`scheduler_wake_jitter_misses`, but they are not latch visual misses by
themselves because the FPGA consumes the already-posted hidden buffer at vblank.
The report also includes latch copy/post/status timings, latch deadline margin,
and finalization timing (`frame_finish_us` plus `post_finish_tail_us`). In latch
mode, benchmark trace rows are buffered during the hot path so periodic TSV
flushes do not masquerade as TV-visible frame skips.

`drop_count=0` from passive `fpga-latch-report` proves that the FPGA accepted
the posted buffers. Combined with zero latch deadline misses, alternating
buffers, and consistent sampled flip-counter deltas, it is the latch visual
smoothness signal. Use passive `fpga-latch-report` for before/after FPGA counters;
`fpga-latch-post-report` posts a diagnostic latch request and can change the
counters it reports. The shared `selected` and `visual_index` columns continue
to describe the Arcade list. Home acceptance uses the dedicated `home_screen`,
`home_menu_token`, `home_selected_token`, `home_selected_index`,
`home_scroll_x`, `home_scroll_max`, and `home_pan_present_active` columns.
Identity tokens are stable FNV-1a values, avoiding per-frame taxonomy string
allocations. The root contract requires focus identity/index changes and
repainted frames. Named submenu contracts additionally require a non-zero
scroll extent, changing scroll position, held input, and active pan frames. An
accepted submenu trace must reach both ends and reverse direction. An idle trace
therefore fails rather than being accepted as smooth motion. Use the
log/status `bench_scenario=home-repeat-hold` fields to confirm the Home
benchmark path. The
default `MISTER_CATALOG_REFRESH=off` isolates Home-row pacing from catalog
refresh noise; pass `--catalog-refresh default` when deliberately measuring the
normal startup mix.

Use `home-nav` only for synthetic fixed-period menu-row stepping; it does not
model the real d-pad repeat gate.

## Arcade And Preview Scenarios

Approved arcade scroll scenarios:

- `held-scroll` - normal continuous movement.
- `turbo-hold` - fast synthetic movement that reverses at list edges.
- `human-turbo-hold` - bursty human-like turbo movement with short pauses; use
  this as the pacing regression gate for launcher frame-timing refactors.
- `velocity-scroll` - alias for `held-scroll`.

Deprecated for arcade performance conclusions:

- `list-scroll`
- old `smooth-scroll`
- manual selected-index jumps
- row-by-row or stepwise scenarios
- the old live-framebuffer scroll-present path
  (`MISTER_ARCADE_SCROLL_PRESENT` / `--scroll-present`)

Use these entrypoints:

```bash
scripts/profile-arcade-scroll.sh LABEL --secs 30 --scenario turbo-hold
scripts/profile-preview-scroll.sh LABEL --secs 30 --scenario turbo-hold
scripts/profile-first-preview.sh LABEL --skip-build
scripts/gate-cold-preview-systems.sh LABEL
```

The cold-preview systems gate reports target-list readiness, candidate
discovery, and selected request/decode/apply as separate phases. The 32ms
request-to-apply budget applies only when the trace identifies a real candidate
with an asset key. A system without an available preview candidate is an
explicit `result=skip` with `skip_reason=no_preview_candidate`; it is not a
latency pass. Missing or misordered phases fail the gate. The final
`preview_state_aggregate_tsv` row counts requested, passed, skipped, and failed
systems while retaining `pass=1` compatibility for a run containing only
passes and legitimate skips. Use
`scripts/profile-cold-preview-systems.sh --self-test` to validate the parser and
gate contract without a device.

`profile-arcade-scroll.sh` defaults to `fpga-vblank-latch-hidden`; pass
`--present-backend fb0-dirty` for an explicit fallback comparison. Latch
counter evidence is required only for latch runs, while both modes verify the
requested backend in the frame trace.

`profile-arcade-scroll.sh` is the reproduction gate for boot-entry stutter. Its
default path reboots the MiSTer, starts the launcher on Home, quickly navigates
to Arcade using `MISTER_ARCADE_ENTRY_INPUT_SCRIPT` or the default Right...A
sequence, then starts the timed `turbo-hold` trace in that same launcher
session. Use `--skip-boot-prelude` only for old direct-to-Arcade comparisons;
do not use that shortcut as evidence for the user-visible boot-to-Arcade flow.
`human-turbo-hold` uses the same Main-supervised Arcade entry path and requires
a bench-tools MagiK binary, so use `--deploy-device` when collecting pacing
regression evidence from a fresh commit.
Pass `--fast` with either `--deploy-device` or `--skip-build` to build or verify
the optimized thin-LTO `release` artifact; without it the runner retains the
production `release-device` contract.
It also gates deferred search indexing: catalog publication must precede index
construction, selection must progress while the index is building, and the
index must finish within the 30-second trace. This makes its frame-pacing and
drop counters direct evidence for search-index contention rather than measuring
only the already-prewarmed steady state.
The script emits and enforces `frame_pacing_gate_tsv` for the 60fps/drop-frame
contract and `preview_exact_gate_tsv` for the no-skipped-preview contract.
For `human-turbo-hold`, the pacing gate treats small wall-time jitter above one
60 Hz period as diagnostic rather than failing evidence because the scenario
intentionally mixes bursts, pauses, reversals, and real entry flow. It still
hard-fails any work frame over budget, any wall frame over 33 ms, any
fallback/timeout/error/unknown vsync source, and any non-zero max miss streak.
For other arcade scenarios, the strict wall gate remains unchanged.
The turbo preview runway defaults to 32 previews ahead; use
`MISTER_PREVIEW_TURBO_LOOKAHEAD=64` to reproduce the old aggressive prefetch
behavior, or `MISTER_PREVIEW_TURBO_RUNWAY=0` only as a diagnostic because it
allows stale/empty previews during turbo scroll.

For the "perfect 60fps Arcade preview" work, each single-commit PR must record
before/after device evidence with the same command set. Use labels that include
the PR slice and BEFORE/AFTER state:

```bash
scripts/profile-preview-scroll.sh LABEL-FADE-TURBO --skip-build --secs 30 --scenario turbo-hold --visual-captures 0
scripts/profile-preview-scroll.sh LABEL-CPU-FADE-TURBO --cpu-profile --secs 30 --scenario turbo-hold --visual-captures 0
```

The CPU profile command builds/deploys the profiling binary, runs the real
Main-supervised Arcade screen with `MISTER_PPROF=1`, exits after the trace
window so the profiler can flush, and pulls
`build/preview-scroll-profiles/LABEL-CPU-FADE-TURBO-arcade-cpu.svg`.
Its `run_context_tsv` row is marked `runtime_type=profile`,
`binary_scope=profile-launcher-scope`, and `production_restore_required=1`.

Preview-scroll benchmarks synchronously warm the screenshot archive cache before
the benchmark timing window and first launcher step unless
`--skip-preview-warm` is passed. Use warm runs for steady 60fps preview evidence
and cold no-warm runs for screenshot-pack index fast-lane work. Production
preview evidence uses the built-in 200ms fade; transition selection flags were
removed from the release benchmark script. `mega` transition coverage is
experimental only and is not release benchmark evidence.
Production preview composition presents the raw preview layer directly by
default. Set `MISTER_PREVIEW_DIRECT_PRESENT=0` only for cached-path A/B
measurements.
`turbo-hold` ping-pongs between the Arcade list edges so long traces keep
exercising preview selection changes after reaching the bottom.

Acceptance fields for Arcade preview pacing:

- Screenshot previews must be exact or intentionally empty for every sampled
  frame in the benchmark trace. `cache_state` values other than `exact` or
  `empty` are failures, even when frame pacing remains clean.
- The trace must include active production fade samples
  (`transition_effect=fade` with `0 < transition_progress < 1`). A hard cut to
  the final preview is a failure, even when every sampled preview is exact.
- `work_gt_16_7ms` after frame 30 is reported as an outlier count.
- `vsync_source_fallback=0`, `vsync_source_timeout=0`,
  `vsync_source_error=0`, and `max_vsync_miss_streak=0`.
- `p99_work_us < 14500` for the preservation-of-fade milestone.
- `profile-arcade-scroll.sh` hard-fails this contract through
  `frame_pacing_gate_tsv`; the p99 work threshold can be overridden with
  `MISTER_ARCADE_SCROLL_P99_WORK_US` for diagnostic comparisons only.
- For render-contract, framebuffer-format, route, or copy-helper changes, use
  `scripts/bench/analyze/launcher-present-trace.py compare BEFORE.tsv AFTER.tsv` and report
  the `present_path_tsv` rows. `cached_present_us`, `arcade_list_present_us`,
  and `fb_present_us` p95/p99 must stay within +5%, and `rows` p95/p99 must not
  increase by more than one row.
- Visual captures must preserve the current fade appearance when enabled.

After the preview fade optimization work, run the final release gate with:

```bash
scripts/gate-preview-60fps.sh LABEL --skip-build --visual-captures 0
```

The gate is for release-candidate confirmation. Per-commit scroll evidence uses
the shorter targeted 30s `turbo-hold` profile above, then the gate can be run
with `--secs 30` when a combined preservation check is useful. It fails if a
trace has non-vsync pacing sources, non-zero max miss streak, any non-exact
preview cache state, or p99 work at/above the configured threshold. It reports
`work_gt_16_7ms` separately so isolated scheduler/prepare-wall outliers can be
investigated without hiding p99 headroom. Pass `--baseline-label BASE` when
validating a before/after change so the gate also fails on present-path
regressions in the copied RGB565 rows. Its parser self-test is:

```bash
scripts/gate-preview-60fps.sh --self-test
```

These scripts write `/media/fat/mister-magik/launcher.env`, send
`mister_magik_restart_launcher`, and lock the real launcher on Arcade with:

```text
MISTER_LAUNCHER_START_SCREEN=arcade
MISTER_LAUNCHER_LOCK_SCREEN=arcade
MISTER_LAUNCHER_BENCH_SCENARIO=held-scroll|turbo-hold|preview-step-hold|preview-idle|idle
MISTER_PREVIEW_SCROLL_TRACE_SECS=N
MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM=1  # only for cold fast-lane benchmarks
MISTER_CATALOG_REFRESH=default
```

Launcher velocity scenarios and preview scroll TSVs require a MagiK binary
built with `--bench-tools`; production `ui` builds intentionally ignore
`MISTER_LAUNCHER_BENCH_SCENARIO` and omit trace writers.

Cold direct-to-system turbo preview readiness is measured with:

```bash
scripts/gate-cold-turbo-preview.sh LABEL --systems arcade,neogeo,saturn --secs 10
```

This gate reboots per system, starts the launcher directly with
`MISTER_LAUNCHER_START_SYSTEM=<system>`, skips full archive warming, enables the
64-item turbo preview runway, and runs `turbo-hold`. It fails if any turbo
selection sample with a preview-capable candidate is blank, stale, failed, or
shows another asset key. Passing rows report `miss_count=0`, `blank=0`,
`stale=0`, `archive_mem_loads=0`, and first selected loads from `index_pread`.
Use `scripts/profile-cold-turbo-preview.sh` without `--require-pass` when
collecting reporting-only data.

Arcade benchmark scripts use `MISTER_CATALOG_REFRESH=default`, not `off`.
Warm startup reads the V3 registry and Arcade mini-nav without hydrating other
systems. `off` is invalid when the scenario requires validation or rebuild
work. Set `on` or `force` only when intentionally benchmarking a rebuild.

Warm validation includes both the root stamp and the discovery checkpoint. The
unchanged path must stay under the existing 2s hard gate including checkpoint
load, compute, compare, drift classification, and worker decision. Coarse
`catalog_checkpoint_tsv` and `catalog_drift_tsv` rows are always emitted. Set
`MISTER_CATALOG_TRACE=detail` only for diagnostics that need per-core
`catalog_profile_manifest_tsv` rows.

Preview transition policy:

- Default real-app preview transition is fixed 200ms `fade`.
- Add new transition experiments under `scripts/experiments/preview/` and
  experiment builds rather than replacing the production `fade` effect.
- For visual review, use `MISTER_LAUNCHER_BENCH_SCENARIO=preview-step-hold`.
- For first selected screenshot latency, use `scripts/profile-first-preview.sh`;
  it runs `preview-idle` so the initial selected result can apply without being
  superseded by a scroll step.

For screenshot pack codec experiments, recode the installed/device-derived
packs. Do not build from the full source screenshot pool unless the experiment
explicitly needs a publish-sized corpus; arcade decode conclusions should use
the same roughly 800-900 entries visible to MiSTer MagiK on the device.

```bash
scripts/magik-cloud run -- cargo run --quiet -- pack-recode \
  --variant mmlz4b-v2-lz4-hc-9-pixels \
  --input /ABS/PATH/device-packs/arcade-screenshots-320x320.mmlz4b \
  --output /ABS/PATH/variants/mmlz4b-v2-lz4-hc-9-pixels/arcade-screenshots-320x320.mmlz4b
scripts/mister put \
  /ABS/PATH/variants/mmlz4b-v2-lz4-hc-9-pixels/arcade-screenshots-320x320.mmlz4b \
  /media/fat/mister-magik/bench/arcade-v2-hc9-pixels.mmlz4b
scripts/profile-preview-pack-decode.sh LABEL \
  --variant mmlz4b-v2-lz4-hc-9-pixels \
  --pack /media/fat/mister-magik/bench/arcade-v2-hc9-pixels.mmlz4b
scripts/profile-preview-scroll.sh 60 turbo-hold LABEL --skip-build --visual-captures 0
```

The decode benchmark reports per-entry `decode_us`, `decode_cpu_us`,
`raw565_parse_us`, `raw565_parse_cpu_us`, `total_us`, `encoded_bytes`,
`decoded_bytes`, and pack-size rows for arcade, Neo Geo, and Saturn when the
matching `--arcade-pack`, `--saturn-pack`, and `--neogeo-pack` paths are
supplied. `decode_us` is monotonic wall time around only the decompressor call;
`decode_cpu_us` is Linux thread CPU time around the same call. Treat large
wall-only outliers as scheduler/device noise unless the CPU column moves with
them. The turbo launcher benchmark emits `warm_gate_tsv`; codec evidence is
invalid unless `loaded=1` and `valid=1` show that the full screenshot archive
was warmed before the 60 second timing window.

Historical evidence:

- `history/2026-6-8/arcade-band-copy-trial.md`
- `history/2026-6-11/rgb565-raw-preview-bench.md`
- `history/2026-6-13/preview-zstd-archive-bench.md`
- `history/2026-6-14/arcade-preview-identity-regression.md`
- `history/2026-6-26-screenshot-codec-bench.md`

## Preview Cache Policy

Original arcade screenshots live on the MiSTer under:

```text
/media/fat/_Arcade/media/screenshot
```

Generated MagiK screenshot packs live under:

```text
/media/fat/mister-magik/assets
```

Only generated cache directories should be deleted/recreated. Runtime preview
loading is raw565-oriented; build caches and publish-ready packs from the Mac in
the private `private/magik-cloud` submodule with:

```bash
scripts/magik-cloud run -- scripts/build-arcade-screenshot-pack.sh --asset-keys-file /ABS/PATH/arcade-asset-keys.txt
scripts/magik-cloud run -- scripts/build-neogeo-screenshot-pack.sh
scripts/magik-cloud run -- scripts/build-console-screenshot-pack.sh --system saturn --input data/sources/saturn/canonical
```

The Arcade pack must be built from the deployed launcher catalog keys. Do not
publish `--all-mame-families` output; that mode is only for diagnostic
full-source experiments and does not match the normal MiSTer MagiK arcade list.

`magik-cloud` writes resized PNGs, `.rgb565` files, and production
`mmlz4b-v2-lz4-hc-9-pixels` archives into ignored local artifact roots. Runtime
preview loading uses the archive path and asset key projected by the catalog; it
must not derive cache paths from PNG/JPG screenshot locations.

The preview loader reads each configured v2 pixel archive into memory when it
opens the archive. Production screenshot packs also publish `.mmlz4b.idx`
sidecars so cold selected and prefetch preview requests can `index_pread` one
payload and show the first screenshot before full-pack warm finishes. Once the
full pack is loaded, steady-state requests return to `archive_mem`.
There is no runtime fallback to PNG/JPG sources or individual `.rgb565` files.
The arcade pack measured on the MiSTer at 34,623,433 bytes takes multiple
seconds to cold-read from `/media/fat` into RAM, so first-preview claims must
state whether `load_source` was `index_pread` or `archive_mem`.

The library scanner must not walk screenshot/cache media directories, read
`gamelist.xml`, or probe normal PNG/JPG screenshots for metadata.

Runtime screenshot-pack downloads are selective: the catalog scan announces the
first discovered supported system, and the media worker checks/downloads only
those packs. Cached-catalog boots seed the same selective requests from the
ready catalog's installed systems after the first visible frame and after active
Arcade/launch interaction settles, so deleting packs without changing the
catalog still re-checks needed packs. Production runs one active pack download
at a time to avoid stealing network, CPU, and SD-card headroom from interaction;
the active pack may still fetch its small index sidecar in parallel. The
catalog-build screen is sourced from structured download/save progress events
rather than parsed log text. The visible download phase streams the raw pack to
`/tmp/mister-magik-media-download` and hashes those bytes there; the later
save/publish phase copies the verified staged file to `/media/fat` and performs
the atomic sync/rename.

Use `scripts/profile-screenshot-download.sh` to measure network download,
verify, save/publish, and total wall time:

```bash
scripts/profile-screenshot-download.sh MEDIA-DL-YYYYMMDD --system neogeo --iterations 1 --replace-label
```

The TSV output is:

```text
screenshot_download_bench_tsv	label	system	variant	encoded_bytes	decoded_bytes	download_ms	decompress_ms	save_ms	verify_ms	total_ms	wire_mbps	decoded_mbps	etag	content_encoding	cf_cache_status	result
```

Use `scripts/profile-screenshot-save.sh` to measure save-progress overhead
separately from network and checksum cost:

```bash
scripts/profile-screenshot-save.sh SAVE-PROGRESS-YYYYMMDD --system neogeo --iterations 10
```

The TSV output is:

```text
screenshot_save_bench_tsv	label	system	mode	iteration	bytes	copy_ms	sync_ms	rename_ms	parent_sync_ms	total_ms	progress_events	result
```

Compare average and p95 `total_ms` plus `copy_ms` when changing production save
behavior. Benchmark claims for screenshot media must state whether they cover
download, decompression, save/publish, verification, and total wall time.

When evaluating media work during Arcade interaction, also run a preview scroll
trace while media requests are pending. Use `frame_pacing` p95/p99 work,
`work_gt_16_7ms`, `preview_latency selected_*_age_us`, and RSS HWM from the log
or status rows. Do not use "still 60fps" as proof; the app can remain vsync
paced while losing CPU or SD-card headroom.

The supported combined production-code gate is:

```bash
scripts/profile-media-arcade-contention.sh MEDIA-ARCADE-YYYYMMDD --deploy-device --secs 60 --replace-label
```

It keeps the installed catalog, starts one real media-download worker and one
`human-turbo-hold` Arcade trace in the same launcher, and automatically records
per-thread `/proc` evidence. Benchmark contention suppresses both the generic
benchmark interaction pause and the Arcade-scroll settling pause; startup,
launch-handoff, low-memory, and all production interaction gates remain active.
The wrapper has an explicit 420-second default timeout and rejects values above
600 seconds. The label-scoped directory under
`build/media-cold-boot/` contains the launcher log, Arcade frame trace, status
snapshots, FPGA latch reports, thread samples, frame-pacing and latch-drop
reports, and `<LABEL>.media-arcade-contention.tsv`. The contention TSV compares
media phase intervals and frame rows using the trace's `startup_elapsed_us`
and absolute `monotonic_us` clocks; it does not infer overlap from the broad
span between the first and last media event. Acceptance requires at least 300
overlapped frames in total, including 180 download frames and 60 publish frames.
At 60Hz these floors prove roughly five seconds of total contention, three
seconds of network/hash work, and one second of exFAT copy/sync/rename work,
rather than a coincidental frame at a phase boundary. Frame pacing is gated on
the generated `<LABEL>.media-arcade-overlap.tsv` subset, not the full run.
The same subset must report `fpga-vblank-latch-hidden` with `ok` status on every
overlapped frame. Completion requires exactly one successful terminal event for
each requested pack, no unexpected or duplicate terminals, no failed pack, and
one worker `Done` event whose pack count exactly matches the request count.

Use `--correctness-only` when validating deterministic all-pack orchestration
independently of the pacing optimization. It still requires successful pack and
worker terminals, operation/frame overlap, one stable supported presentation
backend, bounded completion, and clean teardown. It records preview, thread,
frame-pacing, and latch diagnostics without using those item-9 performance
results to fail the item-5 correctness contract.

The same gate requires at least 10 selected-preview applies during those media
operations, selected-preview apply p99 no greater than 250ms, only
`exact`/`empty` cache states, and zero selected-preview failures. Launcher,
media, and selected-preview thread samples must have non-zero CPU deltas in
sampler intervals that overlap the same boot-monotonic contention window. The
10-apply floor makes the tail statistic meaningful without requiring every
turbo selection to decode instead of hitting the cache. The runner uses a
normal supervised reboot because media/catalog writes and release evidence are
not a fast development-reset case. A run is valid only after the launcher
environment and volatile arming files have been checked clean; `cleanup_tsv`
therefore precedes the final `validity_tsv` row.

For screenshot-pack index work, run both:

```bash
scripts/profile-preview-index-refresh.sh PREVIEW-IDX-DB-YYYYMMDD
scripts/profile-first-preview.sh FIRST-IDX-YYYYMMDD --skip-build
scripts/profile-preview-scroll.sh SCROLL-IDX-YYYYMMDD --skip-build --secs 30 --scenario turbo-hold --skip-preview-warm --visual-captures 0
```

Acceptance evidence should show per-system DB refresh timing from
`preview_index_refresh_tsv`, the first selected decode using
`load_source=index_pread` in the sub-250ms target range, scrolling previews
visible before the full pack load completes, and later steady-state preview
decodes returning to `archive_mem`.

Relevant docs:

- `history/2026-6-13/arcade-screenshot-cache-workflow.md`
- `history/2026-6-14/library-scanner-preview-archive-pruning.md`

## Experiments

Effect-scene profiling and `mega` preview-transition runs are experiments, not
release benchmark evidence. Their scripts live under `scripts/experiments/`,
require an experiment-enabled binary, and are documented in
`docs/experiments/effects.md`.

## Toolchain And Scene Benchmarks

General scene and toolchain benchmark entrypoints:

```bash
scripts/bench-toolchain.sh LABEL --replace-label
mister-magik-fb scenes
mister-magik-fb ui <scene> <secs>
```

`scripts/bench-toolchain.sh` appends formal results to
`history/toolchain-bench/results.tsv`. The TSV keeps the legacy `visual_ok`
column as a combined pass bit and also records `timing_ok` and `capture_ok` so
render/timing failures are distinguishable from agent framebuffer capture
failures. Use `scripts/mister agent framebuffer-capture OUT.png --json
OUT.json` for ad hoc framebuffer PNGs; do not add raw `/dev/fb0` dump or
host-side raw-to-PNG capture paths. For live desktop inspection and FPS
experiments, use the producer-side framebuffer stream (`framebuffer_stream_v1`)
instead of repeated PNG or raw-frame polling; keep PNG capture for still
artifacts and capture validation.

Use the end-to-end latch-stream gate before changing the production stream
default:

```bash
scripts/gate-framebuffer-stream-55fps.sh LABEL --secs 30 --deploy-device
```

For a bounded smoke check against the currently running launcher, require at
least one real producer frame rather than accepting the agent handshake and
heartbeats alone:

```bash
scripts/gate-framebuffer-stream-55fps.sh --smoke
```

Measure sustained resolution capability separately from that cadence gate:

```bash
scripts/profile-framebuffer-stream-resolution.sh LABEL --secs 30 --deploy-device
```

Measure the exact production RGB565 scalar decimator without starting the
Slint UI:

```bash
scripts/mister run "/media/fat/mister-magik/mister-magik-fb framebuffer-stream-scalar-bench"
```

Build and deploy a release `--bench-tools` binary first. The command measures
contiguous, padded, and odd inputs, and checks their deterministic checksums.
The July 10 search retained the tiny C scalar helper only after two exact Rust
production implementations measured materially slower on MiSTer. The slower
NEON experiment and its dedicated gate were removed after that result.

The resolution matrix runs matched half, full, and adaptive null-drain/display
profiles and reports payload, transport, applied/rendered cadence, producer
snapshot cost, coalescing, and the existing latch gate result. Its additional
motion/settle profile requires at least one exact 960x540 adaptive refinement
no more than 15ms after its scheduled refinement deadline. Full-resolution
throughput is reported capability; it is not a prerequisite for adaptive
half-resolution motion.

The gate runs three otherwise identical Arcade `turbo-hold` profiles: no
subscriber, adaptive null drain, and adaptive Analytics display. The desktop
display measurements include a three-second warmup before the requested
measurement interval. Each device motion trace runs 25 seconds longer than that
interval so desktop process and connection setup happen before the measured
window ends.
The Analytics consumer is the release-profile compiled Slint UI with the Skia
renderer and `SLINT_BACKEND=winit-skia`; debug, live-interpreted, or alternate
backend numbers are diagnostic only. The benchmark installs its rendering
notifier after `show()`, requests the first redraw, and accepts notifier
readiness only after `RenderingSetup`. A run with no applied-image
`AfterRendering` callback remains invalid even when receive/apply counters move.
Passing requires at least 55 distinct `AfterRendering` and applied frames per
second in Analytics, at least 58 received/display and null-drain frames per
second, render p95 no greater than 50ms, the native macOS display-link clock, a
working Slint rendering notifier, render interval p95 no greater than 20ms, no
gap over 34ms or consecutive gaps over 20ms, at least 27 rendered serials in
every complete 500ms bucket, at most 10% desktop coalescing, the scalar
decimator, half-size snapshot p95/max no greater than 4/6ms, and full-size
snapshot p95/max no greater than 10/15ms. The underlying profiles also retain
the normal latch, pacing, and zero-drop gates.
Because constant `turbo-hold` is needed to measure sustained stream throughput,
the stream gate selects the explicit `vsync-integrity` pacing policy: normal
60Hz scheduler jitter is diagnostic, while work over budget, wall frames over
33ms, fallback/error sources, latch misses, and non-zero FPGA drops still fail.

The generated `*-framebuffer-stream.tsv` files report consumer measurements;
the matching `*-arcade-scroll.log` files contain
`framebuffer_stream_snapshot_tsv` producer timing. `rendered_fps` specifically
means distinct stream image serials observed at Slint `AfterRendering`, not
frames received, decompressed, applied, or merely submitted for redraw. Winit
redraw events are diagnostic-only because Slint's macOS CADisplayLink path can
draw without emitting `RedrawRequested`. Keep the
historical roughly-20fps monitor result as context only: it predates the latch
producer and is not a like-for-like baseline for this gate.

Current rollout evidence and blockers are recorded in
`history/2026-07-10-framebuffer-stream-cadence.md`. In particular, a valid
30-second full-resolution null drain sustained 56.01fps but had an 89.3ms
interval p95, while half resolution sustained 59.90fps with a 24.4ms interval
p95. These are capability measurements, not permission to enable production.

Build profiles and toolchain details live in `magik-gui/BUILD.md`.

Bench scene documentation lives in `magik-gui/ui/bench/README.md`.

## Catalog Benchmarks

Use the V3 acceptance, first-scan, contention, and standalone rebuild tools:

```bash
scripts/profile-first-scan.sh LABEL --deploy-device --replace-label
scripts/device-catalog-acceptance.sh LABEL
scripts/profile-catalog-contention.sh LABEL --skip-build
scripts/bench-catalog-rebuild.sh LABEL
scripts/mister catalog
```

`device-catalog-acceptance.sh` verifies the active manifest, binding, state,
scanner cache, every per-system SQLite/mini-nav pair, summed totals, and the
absence of V2 artifacts. `scripts/mister catalog` exposes the same read-only V3
integrity report.

`profile-first-scan.sh` moves the complete V3 catalog aside, syncs, and reboots with
the normal `scripts/mister reboot-wait` path. It collects canonical UI markers
from `/tmp/mister-magik/events.jsonl` and embedded-builder timing rows from the
`mister-magik-fb` launcher log. Standalone builder evidence is collected only by
`profile-catalog-builder.sh`. It records first-frame/catalog-ready timings in
`history/toolchain-bench/results-first-scan.tsv`. Historical timing and byte
budgets are comparison-only unless explicitly enabled. Corpus correctness
always uses the V3 inspector. For cold catalog
UX, prefer
`bootstrap_counter_sustained_climb` over the first
`bootstrap_counter_climb`: the latter is only the first meaningful target
(`Games found: 50`), while the sustained metric marks the point where enough
real bootstrap count has reached the UI to keep the visible counter moving.
`full_scan_counter_climb` should mean the classifier count has overtaken the
currently displayed bootstrap count, not merely that classification reported its
first small batch.
`counter_plateau` is derived as
`full_scan_counter_climb - bootstrap_counter_sustained_climb`; use it as the
first-scan "felt stuck" metric when changing bootstrap progress or scanner
progress reporting. `catalog_worker_ram_catalog` records the staged in-memory
catalog projection cost and must be reported separately from scan time and V3
publication time.

For cold-scan retention decisions, judge scanner optimizations against
`library_scan_complete`, `scan_stage_walk`, `scan_stage_file_discovery`, and
`scan_stage_classify_total`. Do not count `library_db_saved`,
`import_stage_total`, SQLite publish, or saved-catalog hydration toward scanner
speedup claims. Non-UX scanner changes should save at least 8s on cold
`profile-first-scan.sh` runs against the relevant baseline before they earn
their complexity.

The V2 destruction, drift, SQL, and monolithic-library runners are retired and
must not be used as release evidence. V3 fault testing operates on immutable
generations and registry slots with bounded restoration. Never benchmark by
directly killing `mister-magik-fb`; that can leave Main and display/OSD state out
of sync.

## Warm Catalog Startup

Use startup reveal acceptance to measure registry-first startup, eager Arcade
mini-nav hydration, validation handoff, and input readiness over five starts:

```bash
MISTER_STARTUP_REVEAL_MODE=warm scripts/device-startup-reveal-acceptance.sh LABEL
```

For warm-start claims, report reveal, first-frame, input-ready, registry load,
Arcade mini-nav load, and validation timings. The harness must not open every
system shard or any V2 artifact.

## Launch Handoff

Use launch-handoff benchmarks when changing launch preparation, FIFO/Main
handoff, or launch failure recovery:

```bash
scripts/profile-launch-handoff.sh LABEL --replace-label --iterations 5
scripts/profile-launch-prep.sh LABEL-WARM --replace-label --scenario warm --iterations 5
scripts/profile-launch-prep.sh LABEL-COLD --replace-label --scenario cold --iterations 3
```

`profile-launch-handoff.sh` writes
`history/toolchain-bench/results-launch-handoff.tsv` rows with:

```text
label	iteration	launch_action_to_loading_us	max_frame_gap_us	loading_frames_before_result	failure_recovery_us	prepare_us	handoff_us	result
```

The target metric is launcher responsiveness during the blocking handoff path:
`max_frame_gap_us` and `failure_recovery_us` should improve or remain within the
existing frame budget while `profile-launch-prep.sh` p95 does not regress.
