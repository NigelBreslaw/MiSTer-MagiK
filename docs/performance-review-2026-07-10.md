# Production Performance Review — 2026-07-10

Scope: production MiSTer MagiK code at source HEAD `00236d8`, the maintained
Main_MiSTer fork at `daf99a8`, and the reference dual-core Cortex-A9 MiSTer.
Experimental effects, effect scenes, `--all-scenes`, and video-lab fallback
paths are excluded.

This review was performed in two phases:

1. A code-only review split across renderer/UI, catalog/exFAT, and
   dual-core/runtime workstreams.
2. Real-device production profiling, including frame/latch gates, preview
   coverage, first scan, CPU/core sampling, SD-card I/O, pack decode, launch
   preparation, analytics, framebuffer scaling, and video/audio playback.

## Executive conclusion

The production renderer is no longer the main performance risk. Home and
Arcade sustain the latch contract with zero visual misses and zero FPGA drops,
and production video sustains 60 fps with audio. The dominant problem is the
catalog lifecycle.

The real first scan failed both release gates:

- RAM catalog usable: **100.634 s**, gate **57.094 s**.
- durable database saved: **193.599 s**, gate **72.573 s**.

The cause is not primarily the final exFAT write. Publishing the completed
11.48 MB database took only **0.974–1.981 s**. The large costs are:

- **46.321 s** discovering active profiles before the real walk;
- **42.468 s** walking the same library again;
- **43.244 s** classification, overlapped with the walk;
- **44.526 s** materializing Arcade UI projections;
- **10.919 s** materializing launch plans;
- **10.034 s** inserting game rows;
- **5.658 s** recomputing and inserting the catalog stamp;
- **9.186 s** constructing the usable in-memory catalog after the scan.

The most valuable optimization is therefore architectural: build one set of
filesystem facts and one finalized catalog projection, then reuse them for
profile activation, classification, RAM navigation, SQLite rows, the stamp,
and saved projections. Eliminating the measured duplicate 46.3-second profile
walk alone would reduce the observed scan from about 89.6 seconds to about
43.2 seconds and puts RAM readiness near the current release gate before any
further dual-core work.

The second major finding is that the intended foreground dual-core policy is
not actually implemented. `ThreadAffinity::Any` returns `"skipped"`; it does
not widen an inherited mask. Main is pinned to CPU1, the catalog worker first
pins itself to CPU0, and foreground children inherit CPU0. In the first-scan
sample, both `library-catalog` and `library-walker` ran **exclusively on CPU0**.
The UI ran exclusively on CPU1. The project is partitioning the cores, but it
is not letting the foreground pipeline use both of them as intended.

## Baseline and validation

### Host correctness

All relevant source tests passed before hardware profiling:

| Command | Result |
|---|---:|
| `scripts/dev-rust test` | 231 passed |
| `cargo test --manifest-path crates/catalog/Cargo.toml` | 303 passed |
| `cargo test --manifest-path apps/mister/Cargo.toml --features ui --no-default-features` | 498 passed |

### Device contract

- 960×540 RGB565 software UI, 1,036,800-byte framebuffer.
- FPGA latch present path, Main-managed scan-out.
- Main and ordinary launcher process start with `Cpus_allowed_list: 1`.
- 59,228 launcher games across 69 systems after full hydration.
- Production binary restored after testing: release-device,
  `ui,bench-tools`, 7,041,220 bytes.
- No destructive reset-fault mode, persistent reset arming, effect scene, or
  raw framebuffer capture was used.

## Phase 2 results

### Frame and interaction profiles

| Profile | Important result | Verdict |
|---|---|---|
| Idle Home, 20 s | 0.50% of one core; 40.5 MB RSS | healthy |
| Home max scroll, 30 s | 1,763 frames; p99 work 8.69 ms; min latch margin 3.944 ms; zero latch/visual/FPGA drops | pass |
| Arcade held, 30 s | p99 work 2.83 ms; min margin 12.911 ms; zero latch/visual/FPGA drops | latch pass; strict wall-jitter gate failed |
| Arcade turbo, 30 s | p99 work 4.02 ms; max work 13.576 ms; min margin 3.039 ms; zero latch/visual/FPGA drops | latch pass; strict wall-jitter gate failed |
| Arcade human | navigation input-to-present 501 ms | entry gate failure |
| Arcade human, relaxed entry gate | 1,795 exact and 4 empty preview samples; p99 work 4.00 ms | diagnostic pass, correctness miss retained |
| First selected preview | request-to-apply 8 ms; decode 1.251 ms; total 2.570 ms; 478/478 exact | pass |
| Held preview gate | 1,791 exact, 8 non-exact; p99 work 2.467 ms | exactness failure |
| Turbo preview | 1,795 exact, 4 non-exact; p99 work 4.190 ms; queue age p99 113.7 ms, max 147.1 ms | exactness failure |

Interpretation:

- The FPGA latch and RGB565 copy design are sound. Ordinary scheduler/vsync
  wall jitter produced ~16.8 ms p99 and occasional ~33 ms intervals, but did
  not create latch deadline misses or FPGA drops. Optimizing low-work wall
  jitter is lower priority than the correctness and catalog findings.
- Arcade's direct layers are effective. Hidden composition was about 1.3 ms
  and steady work remained far below budget.
- Home motion is materially heavier: Slint render was roughly 6.8 ms and a
  typical active pan copied 777,728 bytes. It passes today, but it has much less
  headroom than Arcade.
- Preview decode speed is adequate; the failure is scheduling/coverage, not
  codec throughput.

### Preview pack and media indexes

The bounded production pack profile decoded 600 Arcade samples in catalog
order:

| Metric | Result |
|---|---:|
| LZ4 decode p50 / p95 / p99 / max | 621 / 1,154 / 1,379 / 1,434 µs |
| RGB565 parse p99 | 1,142 µs |
| total p99 | 2,371 µs |
| average encoded / decoded bytes | 28,652 / 159,363 |

Installed immutable pack sizes were 24.53 MB Arcade, 9.07 MB Saturn, and
4.97 MB Neo Geo. These coarse packs are a good fit for exFAT; the sidecar
`pread` path should remain the production design.

`preview-index-refresh-bench` failed all eight configured systems. Its CLI
SQLite connection does not register the `magik_path` SQL function, so every
query failed with `no such function: magik_path`. This is a benchmark/runtime
integration bug and prevents valid index-refresh timing claims.

### Warm startup and validation

Five warm restarts were consistent:

| Metric | Range |
|---|---:|
| first frame | 83–84 ms |
| summary load | 38.15–38.49 ms |
| full navigation ready | 5.335–5.361 s |
| navigation projection load | 1.169–1.181 s |
| projection open | 0.813–0.825 s |
| catalog object hydration | 0.354–0.355 s |

The first frame already has a usable summary and is excellent. Full hydration
is intentionally deferred, but opening the navigation projection from exFAT is
still expensive.

Warm unchanged validation is much worse than its name implies. Observed stamp
checks took **3.22–8.58 s**, almost all in checkpoint/stamp computation, while
opening and reading the stored stamp took only milliseconds. Warm validation
continues to enumerate depth-two filesystem facts instead of reusing a durable
fact snapshot or processing a delta.

### First scan and storage

The production first-scan run, with per-thread sampling, produced:

| Stage | Time |
|---|---:|
| first frame | 95 ms |
| active profile discovery | 46.321 s |
| real filesystem walk | 42.468 s |
| file discovery bookkeeping | 16.280 s |
| classification | 43.244 s |
| scan complete | 91.145 s |
| RAM catalog construction | 9.186 s |
| RAM catalog ready | 100.634 s |
| metadata load for import | 2.987 s |
| insert games | 10.034 s |
| materialize Arcade UI | 44.526 s |
| insert console launcher rows | 3.024 s |
| materialize launch plans | 10.919 s |
| insert catalog stamp/checkpoint | 5.658 s |
| total SQLite build stage | 79.156 s |
| publish 11.48 MB database | 0.974 s |
| database saved | 193.599 s |

The independent library I/O profile completed in 175 seconds and corroborated
the same shape: 30.6 seconds of scan, 30.9 seconds materializing Arcade UI,
8.4 seconds creating launch plans, 6.9 seconds inserting games, 4.3 seconds
writing the stamp, and 1.98 seconds publishing the database.

The first scan's core evidence is decisive:

| Thread | CPU jiffies | Observed processor |
|---|---:|---|
| `library-catalog` | 12,058 | CPU0 for 147/147 samples |
| `library-walker` | 1,867 | CPU0 for 36/36 samples |
| launcher/UI main | 2,573 | CPU1 for 149/149 samples |
| preview prefetch | 0 | CPU0 |
| selected preview | 0 | CPU1 |

`library-catalog` used about 120.6 CPU-seconds in the sampled interval. The
walker used another 18.7 CPU-seconds. Both were forced to compete on CPU0,
while CPU1 mostly serviced a scan overlay that rendered at 60 Hz.

### Launch preparation and handoff

Launch preparation itself is mostly cheap:

| Scenario | Result |
|---|---:|
| warm, 60 refs | p50 14 µs, p95 4.721 ms |
| cold, 36 refs | p50 18 µs, p95 2.071 ms |
| structured Neo Geo refs | typically 9–59 µs |
| AmigaVision descriptor creation | roughly 1.9–4.8 ms, one 4 KiB write each |

The long whole-command runtime came from sweeping many references and millions
of small logical reads, not an individual normal launch. Prewarming immutable
descriptors and avoiding tiny exFAT writes remains worthwhile for AmigaVision.

The launch-handoff scenario emitted no samples. It entered `acornatom` with an
empty first row and remained idle, so this is a fixture-selection failure, not
a launch latency result.

### Analytics and framebuffer streaming

- Idle analytics baseline, wall, thread, and process modes all measured 0.50%
  launcher CPU and 0.00% agent CPU at the profile's resolution. The volatile
  lease was cleaned up.
- The RGB565 half-scale scalar benchmark was correct for full, padded, and odd
  strides. p95 was 1.022–1.028 ms; max was 1.112–1.169 ms.
- The current producer still snapshots by reading the hidden framebuffer
  mapping on the UI thread before a low-priority worker compresses it. Existing
  same-day stream evidence reports much larger full snapshot tails than the
  scalar downsampler itself, so eliminating write-combined framebuffer
  readback is the relevant optimization.

### Production video/audio

The maintained production configuration was built and run exactly as shipped:
direct blit, source scale, `custom-neon`, queue depth 2, no decoder threading,
and no video-lab features.

| Metric | Result |
|---|---:|
| output | 1,800 frames / 30.0 s = 60.0 fps |
| vsync | 1,800 hits, zero timeout/fallback/error |
| audio underruns | 0 |
| process CPU | 52.5% of one core final; 50% host sample mean |
| main/UI CPU | 2.7% |
| decode-thread CPU | 48.0% |
| video packet decode p50 / p99 | 7 / 22 µs |
| I420→RGB565 conversion p50 / p95 / p99 | 3.301 / 3.427 / 4.036 ms |
| conversion max | 5.568 ms |
| framebuffer present p50 / p99 | 266 / 379 µs |
| wall p99 / max | 16.765 / 22.395 ms |

Conversion, not H.264 decode, dominates. The runtime label says
`custom-neon`, but dispatch is compile-time under
`cfg(target_feature = "neon")`, and the ARM build emits a warning that
`+neon` is unstable. Add a runtime/backend identity row or a disassembly gate;
the configuration string alone does not prove the NEON branch exists in the
binary.

## Phase 1 findings and optimization plan

### P0 — Give affinity states explicit semantics

Code: `crates/catalog/src/runtime_thread.rs:164`.

`ThreadAffinity::Any` currently means “make no system call.” Those are not the
same semantics when the caller inherits CPU0 or CPU1. Replace the two-state API
with explicit states such as:

- `Inherit` — deliberately keep the parent mask;
- `AllOnline` — set all online Cortex-A9 CPUs;
- `Cpu0` and `Cpu1` — explicit isolation.

Use `AllOnline` for the foreground catalog coordinator and walker. Preserve
CPU0+niced policy for background catalog, prefetch, media, stream, and agent
work. Preserve the high-priority UI on CPU1. Let Linux schedule foreground
walker/classifier work around the UI rather than trapping both catalog threads
on CPU0.

Gate: repeat first scan with thread sampling. Require both catalog threads to
show a `0-1` allowed mask and actual residency on both cores, with no Home or
Arcade latch regression.

### P0 — Eliminate the duplicate cold filesystem traversal

Code:

- `crates/catalog/src/launch_profiles.rs:328`
- `crates/catalog/src/launch_profiles.rs:468`
- `crates/catalog/src/library_indexer.rs:132`

`active_profiles_for_roots` discovers installed cores, enumerates unclaimed
top-level game directories, and performs shallow payload checks. The real
scanner then walks the activated targets again. On this card the first phase
cost 46.3 seconds and the second 42.5 seconds.

Create a `ScanPlan`/`DiscoverySnapshot` that owns:

- installed core facts;
- top-level game directory headers;
- shallow payload evidence needed for runtime profile decisions;
- resolved active profiles and targets;
- reusable directory entries for the subsequent walker where safe.

The walker should consume this plan instead of reopening the same paths.
Preserve deterministic ordering and existing audit semantics.

Gate: `active_profiles` should become derivation time rather than I/O time.
Demand at least an eight-second cold-scan win; the measured opportunity is
approximately 46 seconds.

### P0 — Stop deriving the catalog repeatedly during SQLite publication

Code: `crates/catalog/src/sqlite_catalog.rs:2620-2727` and
`crates/catalog/src/catalog_projection.rs:275`.

The scanner already has discoveries and builds a RAM catalog, then SQLite
re-derives UI rows and launch plans with expensive SQL materialization. Build
one finalized, interned projection in RAM and use it for all three consumers:

1. immediate launcher/navigation readiness;
2. bulk SQLite insertion;
3. summary/navigation projection serialization.

Specific changes:

- deduplicate and enrich each discovery once;
- construct launch plans once, not from a later SQL traversal;
- feed Arcade and console launcher rows from the same finalized iterator;
- share path/title/system dictionaries rather than reallocate strings;
- compute stamp/checkpoint from the already-collected discovery snapshot;
- keep SQLite as durable/queryable storage, not a second catalog compiler.

The durable-save gate cannot be met by tuning the final 1-second copy. It
requires removing most of the measured 79-second derivation stage.

### P0 — Honor the intended scan-overlay redraw budget

Code: `apps/mister/src/ui_runner/launcher_loop.rs:2602-2640` and
`apps/mister/src/ui_runner/launcher_loop.rs:2833-2868`.

`catalog_scan_redraw` computes a reduced cadence, but
`CATALOG_SCAN_VISIBLE` is itself an unconditional wake reason. The loop
therefore cannot sleep while the overlay is visible and renders it near 60 Hz.
First-scan logs show repeated 3.1–4.4 ms Slint renders at 60 Hz, and the UI main
thread consumed about 25.7 CPU-seconds during the sample.

Make visibility a state, not a wake source. Wake on progress/detail change or
the explicit redraw deadline. A 10–15 Hz scan overlay is sufficient. This
reduces CPU and memory-bus pressure and leaves more thermal/scheduler headroom
for FUSE and catalog work.

### P1 — Replace warm rescans with durable validation facts and deltas

Warm validation spends seconds rebuilding facts to prove that nothing changed.
Persist the normalized inputs needed by stamp/checkpoint comparison:

- root and directory identity;
- installed-core facts;
- top-level game directory summaries;
- metadata database size/mtime/version;
- audit facts and pack identities.

On warm boot, validate cheap directory signatures first, then rescan only
changed subtrees. Reuse one in-memory snapshot for stamp, checkpoint, audit,
and drift detail. A full verification command can remain available for repair
or explicit rebuild.

### P1 — Make every selected-preview miss asynchronous

Code: `apps/mister/src/preview_state.rs:1218-1405`.

Turbo misses use the selected loader; ordinary misses call
`load_asset_pixels_timed` synchronously on the UI thread. The first preview is
fast today, but a cold exFAT/read/decode outlier can still block an ordinary
navigation frame. Route all selected misses through the high-priority selected
worker and retain the previous exact preview until the generation-matched
result arrives.

At the same time:

- replace unbounded drain/backlog behavior at `preview_state.rs:1834` with a
  bounded newest-generation mailbox;
- prioritize one selected result over prefetch results per frame;
- cancel stale queued work aggressively;
- cache `system -> resolved pack path` and invalidate it when media state is
  published.

Gate: zero non-exact samples in held, turbo, and human profiles; selected
queue-age p99 below one navigation interval.

### P1 — Avoid rebuilding the unchanged preview window every frame

Code: `apps/mister/src/preview_state.rs:689-746`,
`preview_state.rs:1015-1024`, and `launcher_loop.rs:2693-2717`.

The launcher calls preview scheduling on every eligible Arcade loop. Even when
selection is unchanged, it hashes several strings across the window and can
rebuild `Vec<String>`/`HashSet<String>` state. Replace this with a cheap
invalidation tuple: catalog generation, system identity, selected index,
radius/direction, and media generation. Recompute keys/signature only when one
of those changes.

### P1 — Skip archive TOC work when a profile treats the archive as the game

Code: `crates/catalog/src/library_indexer.rs:212-230`.

Every recognized archive extension can trigger central-directory parsing even
for profiles whose launch semantics are file-as-game and have no archive-entry
rules. Encode that property in `LaunchProfile` and bypass TOC/ref-stat work.
This is especially relevant to large raw MAME/Neo Geo collections.

### P1 — Make video conversion identity real and reuse frame buffers

Code:

- `apps/mister/src/video_i420.rs:25-59`
- `apps/mister/src/video_player.rs:827-860`
- `apps/mister/build-arm.sh:155`

Each decoded frame allocates a new `Vec<u16>` for the full RGB565 output. Pool
two or three conversion buffers with the queue slots. Add a compile/runtime
backend ID (`neon` or `scalar`) to the benchmark row and CI inspection. If the
stable compiler is not setting the cfg, use an explicit ARM runtime dispatch
or a narrowly contained NEON object/function rather than relying on a warning-
producing global target feature.

The current 3.3 ms average conversion is safe at 320×240 but consumes nearly
half a core with decode/audio. Buffer reuse and verified SIMD are the clearest
ways to add resolution/codec headroom.

### P2 — Keep the working direct-layer renderer and target Home

Arcade direct composition is already efficient. Do not reintroduce full Slint
rendering or wider color modes. The next renderer experiment should focus on
Home pan:

- cache cabinet/text layers that do not change during pan;
- move the pan strip to a direct RGB565 layer if composition invariants permit;
- compare exact-stride versus full-row copies using actual present bytes;
- retain the current latch gate as the deciding metric.

The two latch status reads cost only about 60–77 µs p99 and are not a priority.

### P2 — Stream from the cached composition, not from write-combined fb0

The stream producer should receive the exact cached RGB565 frame or damage
result before the framebuffer write. Full-resolution stream snapshots should
never need to read `/dev/fb0`/the hidden write-combined mapping on the UI
thread. Keep scaling/compression on CPU0 at nice +10 and use a single-slot
latest-frame mailbox.

### P2 — Make exFAT publishing coarse and explicit

The measured final 11.5 MB database copy is acceptable. Optimize exFAT around
metadata and sync behavior:

- immutable pack/index pairs rather than many small media files;
- one temp build in `/tmp`, one sequential copy, one final sync/rename policy;
- prebuilt AmigaVision descriptors or a packed descriptor store;
- no per-row fsync and no small-cache writes on navigation hot paths;
- A/B SQLite page size, cache size, and transaction chunking before changing
  defaults.

The video asset sync reinforced this: ten small MP4 transfers were dominated by
per-file deployment latency. The repository's packed screenshot model is the
right direction.

## Benchmark infrastructure defects found

These should be fixed before using the affected commands as release gates:

1. Several exclusive CLI wrappers invoke their command while the launcher owns
   the process lock. They must use the supported Main
   `mister_magik_suspend`/settle/command/`resume` lifecycle.
2. `profile-launch-prep.sh` can exceed the host command transport timeout and
   leave the device process running after the wrapper returns. It needs a
   remote result file plus bounded polling.
3. `profile-cold-turbo-preview.sh` removes `library.nav.lz4b`, then runs the
   exclusive repair command while the launcher is active. The launcher rebuilt
   the projection, but the harness exited 13.
4. `preview-index-refresh-bench` lacks the `magik_path` SQL function.
5. Launch-handoff and idle-Arcade fixtures assume row zero is launchable and
   preview-bearing. Selection must query a valid row first.
6. Failed strict wall-pacing summaries should distinguish harmless scheduler
   jitter from latch/visual/FPGA failure; both should remain visible, but only
   the latter is a production correctness failure under the current contract.

## Recommended implementation order

1. Fix explicit `AllOnline` affinity semantics and add allowed-mask evidence.
2. Merge profile discovery and the real walk into a reusable `ScanPlan`.
3. Skip archive TOC and ref-stat work for file-as-game profiles.
4. Fix scan-overlay wake cadence; re-run first scan after steps 1–4.
5. Build one finalized catalog projection and bulk-persist it; target the DB
   save gate.
6. Make exFAT publishing coarse and explicit, then A/B its SQLite parameters.
7. Persist/reuse warm validation facts; target sub-second unchanged checks.
8. Make all selected preview misses async and bound/coalesce result queues.
9. Add preview window invalidation and pack-path caching.
10. Pool video RGB565 buffers and prove the selected SIMD backend.
11. Move stream snapshots to the cached composition source.
12. Optimize Home pan only after the catalog and preview correctness gates pass.

## Required gates after optimization

- First scan: RAM catalog ≤57.094 s and DB save ≤72.573 s, with thread/core
  evidence and no persistent launcher environment.
- Warm startup: first frame ≤100 ms, no synchronous full-catalog load before
  reveal, unchanged validation target below 1 s.
- Home/Arcade: zero latch deadline misses, visual misses, FPGA drops, or route
  errors; no regression in p99 work.
- Preview: zero non-exact previewable selections in held, turbo, and human
  profiles; first preview remains within the current 8 ms request/apply result.
- Video: 60 fps, zero underruns/fallback/errors, explicit `neon`/`scalar`
  identity, conversion p99 no worse than 4.04 ms.
- Stream: producer snapshot work excluded from the UI-thread fb0 read path;
  existing full/half cadence and latch gates preserved.

## Evidence locations

- `build/first-scan-profiles/PERF-REVIEW-20260710-FIRST-SCAN.log`
- `build/first-scan-profiles/PERF-REVIEW-20260710-FIRST-SCAN-first-scan-thread-sample.tsv`
- `history/toolchain-bench/results-warm-catalog.tsv`
- `history/toolchain-bench/results-library-io.tsv`
- `history/toolchain-bench/results-launch-prep.tsv`
- `history/toolchain-bench/results.tsv`
- `build/launcher-home-scroll-profiles/PERF-REVIEW-20260710-HOME-*`
- `build/arcade-scroll-profiles/PERF-REVIEW-20260710-ARCADE-*`
- `build/preview-scroll-profiles/PERF-REVIEW-20260710-*`
- `build/preview-pack-decode/PERF-REVIEW-20260710-PACK-DECODE-preview-pack.tsv`
- `build/preview-index-refresh/PERF-REVIEW-20260710-INDEX-REFRESH-preview-index-refresh.tsv`
- `build/analytics-overhead/PERF-REVIEW-20260710-ANALYTICS/analytics-overhead.tsv`
- `history/toolchain-bench/PERF-REVIEW-20260710-VIDEO-video_playback-ui.log`
