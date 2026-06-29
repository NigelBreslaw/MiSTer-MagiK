# Production Performance Deep Review - 2026-06-29

Scope: production code and production benchmarks only. Experimental effects are
excluded. This review looked at the current tree at `ea2057d1`, the last 15
commits, the hot-path code, and a broad real-device benchmark pass on the MiSTer
at `192.168.1.117`.

## Benchmark Provenance

The first hardware pass used the already deployed bench-tools binary because a
fresh local `release-device` rebuild failed on the host with `No space left on
device`. After host space was freed, the clean path was retried successfully:

- Build command:
  `scripts/deploy-rust.sh --device --ui-scope launcher --bench-tools`.
- Built revision: `ea2057d1`.
- Built profile/features: `release-device`, `ui,bench-tools`.
- Built binary size: `6,078,388` bytes.
- Deploy checksum: `8984a8d1d81e93f3`.
- Remote size: `6,078,388` bytes.
- Deploy transport: Main-supervised agent deploy.

The follow-up benchmark commands used `--skip-build` after that successful
deploy, so some scripts still conservatively printed
`binary_scope=deployed-unknown` or `deployment_state=unverified-skip-build`.
Treat the `PERFREVIEW-CLEAN-20260629-*` rows as the fresh-deploy retry set, with
that script-labeling caveat.

Device health after the clean deploy:

- `MiSTer_MagiK` and `mister-magik-fb` were running.
- Framebuffer mode was RGB565, `960x540`.
- Launcher was on Home at 60fps.
- `scripts/mister status` reported a healthy launcher scene.

## Clean Retry Summary

The clean retry confirmed the original conclusions:

- Preview 60fps gate passed.
  - Held scroll: p99 work `3221us`, work frames over 16.7ms `0`, all vsync.
  - Turbo hold: p99 work `3474us`, work frames over 16.7ms `0`, all vsync.
- Warm preview scroll remained healthy: p99 work `3611us`, work misses `0`.
- Cold/no-warm preview scroll remained healthy: p99 work `3663us`, work misses
  `0`, unexpected preview file reads `0`.
- First-preview still hit a catalog-worker frame spike:
  - selected apply age `75479us`;
  - selected decode source `index_pread`;
  - slow frame work `105575us`;
  - `catalog_worker_us=105426`.
- Dedicated Arcade scroll remained healthy:
  - p99 `fb_present_us=1301`;
  - p99 `arcade_list_present_us=730`;
  - all vsync.
- Launch handoff reproduced the issue:
  - success iteration 3 max frame gap `108472us`;
  - slow-fail iteration 3 max frame gap `105236us`;
  - slow-fail recovery stayed about `2.3ms`.
- First scan still failed the RAM catalog gate:
  - `library_ready=44768ms > 41000ms`;
  - DB saved at `52655ms`;
  - scan/classify about `39.47s`;
  - SQLite publish total `655ms`.
- Library I/O isolated run:
  - scan/classify about `22.44s`;
  - import about `9.10s`;
  - SQLite publish total `527ms`.
- Media cold boot with catalog kept:
  - Arcade completed at `23238ms`;
  - Neo Geo completed at `25972ms`;
  - Saturn completed at `30006ms`.
- Screenshot save:
  - Arcade `3192ms`, `1939ms`, `1969ms`;
  - Neo Geo `550ms`, `401ms`, `387ms`;
  - Saturn `1000ms`, `682ms`, `757ms`.
- Screenshot download plus staged save:
  - Arcade total `9728ms`, save `2132ms`;
  - Neo Geo total `2414ms`, save `571ms`;
  - Saturn total `3751ms`, save `745ms`.

## Recent Commit Shape

The last 15 commits are heavily performance/instrumentation oriented:

```text
ea2057d1 Trim optional paths from production binary
37ffac2e perf: make preview direct present default
6a185a1c Add fast reboot diagnostics and direct reset tooling
47495f28 perf: remove rejected dense present experiment
11af599e perf: guard unchanged launcher bridge properties
69aa0321 perf: add dense arcade list present experiment
fb80d700 perf: publish AmigaVision launch descriptors atomically
7249fed7 bench: add launch handoff success mode
32767955 perf: sync media publishes without shelling out
d98a5627 Slim production video build
8f164a31 History
05746314 perf: defer sqlite catalog text indexes
8adb44e2 perf: cache preview archive metadata briefly
71b5765a perf: cancel stale preview prefetch work
fc007123 perf: fast path production preview fade
```

The broad direction is good: smaller production binaries, fewer optional paths,
more observable handoff/media/catalog behavior, direct RGB565 preview present,
less unnecessary bridge churn, safer media and descriptor publishes, and better
benchmark coverage.

## Phase One: Static Review Findings

### 1. Handoff Runtime Polling Can Steal A Frame

Relevant code:

- `magik-gui/src/ui_runner/launch_handoff_session.rs`
  - `runtime_action` probes after 500ms while a handoff is active.
  - `spawn_launch_worker` does not apply a runtime thread policy.
- `magik-gui/src/launcher.rs`
  - `mister_running_arcade_core()` shells through `pidof` and `/proc`.

Why this matters:

The dual-core Cortex-A9 has enough capacity for the launcher when work is
partitioned cleanly, but shell process creation from the UI/runtime path is
expensive and scheduler-hostile. The measured handoff profiles showed repeated
approximately 100ms max frame gaps on iteration 3 in both success and slow-fail
modes, matching this static concern.

Recommendations:

- Throttle the arcade-core probe with an explicit next-probe timestamp.
- Prefer a Main status/fifo acknowledgement over shelling out.
- Add a `LaunchHandoff` runtime thread role, likely CPU0 and low priority.
- Add a handoff gate on `max_frame_gap_us`, not only on final success/failure.

### 2. Catalog Worker Work Still Lands On Visible Frames

Relevant code:

- `magik-gui/src/ui_runner/catalog_worker.rs`
- `magik-gui/catalog/src/sqlite_catalog.rs`
- `magik-gui/catalog/src/catalog_navigation.rs`

Why this matters:

The architecture wisely builds SQLite on tmpfs and publishes to exFAT in large
chunks, but catalog worker messages can still be processed in visible UI frames.
The first-preview run had a single `100814us` work frame attributed almost
entirely to `catalog_worker_us=100601`, even though the selected preview itself
was fast.

Recommendations:

- Give catalog message processing a per-frame budget.
- Coalesce or defer non-urgent catalog notifications during interaction.
- Split "RAM catalog ready" from "durable DB/projections saved" in UI and
  gates.
- Start durable saves and projection repairs only after an idle window.

### 3. Direct Preview Present Is Under-Attributed

Relevant code:

- `magik-gui/src/ui_runner/launcher_compositor.rs`
  - `PresentResult` records cached present, arcade list present, and total
    framebuffer present.
  - The direct preview rect copy is not separately timed.

Why this matters:

Direct preview present is now production default. Current traces can infer its
cost from the remainder, but cannot directly say whether preview copy, arcade
copy, cached Slint copy, or framebuffer mapping dominates a frame.

Recommendations:

- Add `direct_preview_present_us`.
- Add copied pixel/byte counts per present path.
- Report present-path totals in preview and arcade profile summaries.

### 4. Preview Archive Warm Is A Hidden I/O Policy Decision

Relevant code:

- `magik-gui/catalog/src/preview_worker.rs`
  - `index_pread` can satisfy first preview.
  - Then background archive load reads the full `.mmlz4b` into memory.

Why this matters:

The fast lane is good: use the sidecar index for targeted reads. The risk is
that a single preview selection can trigger a full pack read on the background
core. On exFAT SD this is usually fine when idle, but it can compete with
catalog/media work during cold or busy paths.

Recommendations:

- Keep index-only mode longer for rarely used systems.
- Promote full archive memory only after an idle window.
- Treat media generation changes as the invalidation signal rather than frequent
  archive metadata checks.
- Trace archive-warm state separately from selected-preview latency.

### 5. Media Writes Are Large Enough To Deserve Stronger Idle Gates

Relevant code:

- `magik-gui/src/ui_runner/media_worker.rs`
- `magik-gui/src/artifact_publish.rs`

Why this matters:

Recent commits improved media publish by avoiding shell sync paths. The device
runs still show screenshot/media pack saves in the hundreds of milliseconds to
several seconds, dominated by exFAT copy time. New downloads are interaction
gated, but active downloads and publish work can continue after the user starts
interacting.

Recommendations:

- Keep media concurrency at one in production.
- Pause or defer publish when interaction starts, not just new download starts.
- Prefer staged work outside `/media/fat` when practical, then a single large
  publish.
- Add a "media active during interaction" counter to benchmark summaries.

### 6. Arcade Scroll Is Healthy, But Copy Volume Is Still The Next Render Target

Relevant code:

- `magik-gui/src/arcade_list_renderer.rs`
- `magik-gui/src/ui_runner/ui_frame_target.rs`

Why this matters:

The scroll path reuses a circular RAM surface, but the present path still copies
almost the full list layer. Current numbers are well within budget, so this is a
second-order optimization, not a fire.

Recommendations:

- Track rows, pixels, and bytes for list present.
- Test dense list copy versus segmented copy only behind benchmark flags.
- Do not revive live-framebuffer read/scroll paths without new evidence.

### 7. Runtime Thread Policy Is Mostly Working, But Should Be Made Explicit

Relevant code:

- `magik-gui/catalog/src/runtime_thread.rs`

Current policy already places many heavy roles on CPU0 and leaves selected
preview more flexible. Device thread samples showed the intended shape: UI and
vsync mostly on CPU1, catalog/media/prefetch mostly on CPU0.

Recommendations:

- Add an explicit UI/render thread role and benchmark CPU1 pinning.
- Add explicit launch handoff role.
- A/B selected-preview CPU0 versus any-CPU placement, using selected apply age
  and p99 frame work as the deciding metrics.

### 8. Benchmark Tooling Needs A Few Guard Rails

Findings:

- Device acceptance can skip bench-tools by default.
- Some catalog benchmark env defaults differ from documented production policy.
- `profile-arcade-scroll.sh` lacks the run-context labels now present in preview
  scripts.
- First-preview failures can be reported as rows instead of hard gates.
- Launch-prep masks remote failure and has no threshold gate.
- Some framebuffer captures can contaminate a timed process and should be kept
  out of timing-critical sections.

Recommendations:

- Make benchmark run context consistent across scripts.
- Fail hard when validity/gate rows fail.
- Separate visual capture from timed workload unless explicitly requested.
- Keep `binary_scope`, `features`, and deployment verification visible in every
  profile row.

## Phase Two: Hardware Evidence

### Preview Scroll, Warm

Command:

```bash
scripts/profile-preview-scroll.sh PERFREVIEW-20260629-WARM --skip-build --secs 30 --scenario turbo-hold --visual-captures 0 --thread-sample
```

Key results:

- Valid run, all vsync, no fallback/timeout/error.
- Frames after warmup: `1714`.
- Average work: `2741us`; p95 work: `3293us`; p99 work: `3474us`.
- Work frames over 16.7ms: `0`.
- `preview_blit_us` p95: `1626us`.
- `arcade_list_present_us` p95: `663us`.
- `fb_present_us` p95: `1172us`; p99: `1290us`.
- Preview rows: `577`; cache hits: `22`; archive memory: `542`;
  `index_pread`: `12`.
- Unexpected file reads: `0`; slow reads: `0`; selected file reads: `0`.
- Thread samples: UI process mostly CPU1, prefetch mostly CPU0.

Interpretation:

Warm preview browsing is comfortably inside frame budget. Direct preview plus
arcade list copy is not threatening 60fps.

### Preview Scroll, Cold/No Warm

Command:

```bash
scripts/profile-preview-scroll.sh PERFREVIEW-20260629-COLD --skip-build --secs 30 --scenario turbo-hold --visual-captures 0 --skip-preview-warm --thread-sample
```

Key results:

- Valid run, all vsync, no fallback/timeout/error.
- Frames after warmup: `1749`.
- Average work: `2749us`; p95 work: `3418us`; p99 work: `3623us`.
- Work frames over 16.7ms: `0`.
- Preview rows: `1005`; cache hits: `439`; archive memory: `553`;
  `index_pread`: `13`.
- Unexpected file reads: `0`; slow reads: `0`; selected file reads: `0`.

Interpretation:

Skipping preview warm did not create frame-budget failures. The index path and
background warm are working for this scenario, but the I/O policy should still
be tightened for mixed catalog/media/cold paths.

### First Preview

Command:

```bash
scripts/profile-first-preview.sh PERFREVIEW-20260629-FIRST --skip-build --secs 8
```

Key results:

- Selected preview source: `index_pread`.
- Selected decode total: `15004us`.
- Selected read: `353us`.
- Selected decode: `13484us`.
- Selected encoded bytes: `63424`.
- Selected decode queue age: `625us`.
- Selected apply age: `79880us`.
- One work miss: frame work `100814us`.
- Slow-frame attribution: `catalog_worker_us=100601`.

Interpretation:

First selected preview is not the problem. Catalog worker work is still able to
land as a visible 100ms frame event.

### Arcade Turbo Scroll

Command:

```bash
scripts/profile-arcade-scroll.sh PERFREVIEW-20260629-ARCADE --skip-build --secs 30 --scenario turbo-hold --thread-sample
```

Key results:

- Frames: `1744`.
- All frames used vsync path; fallback/timeout/error: `0`.
- `custom_draw_us` p95: `1674us`; p99: `1785us`.
- `fb_present_us` p95: `1218us`; p99: `1368us`.
- `arcade_list_present_us` p95: `663us`; p99: `732us`.
- Wall p95: `16791us`; p99: `17092us`.
- Rows copied p95/p99: `704`.

Interpretation:

Arcade scrolling is healthy. The remaining render opportunity is copy-volume
polish, not emergency remediation.

### Launch Prep

Commands:

```bash
scripts/profile-launch-prep.sh PERFREVIEW-20260629-LAUNCHPREP-WARM --replace-label --scenario warm --iterations 5
scripts/profile-launch-prep.sh PERFREVIEW-20260629-LAUNCHPREP-COLD --replace-label --scenario cold --iterations 5
```

Key results:

- Warm: count `60`, errors `0`, p50 `26us`, p95 `4417us`.
- Cold: count `60`, errors `0`, p50 `35us`, p95 `2520us`.
- Descriptor writes: `20` per run.
- Descriptor bytes: `540`.
- p95 is dominated by AmigaVision descriptor temp/write/sync/rename work.

Interpretation:

Most launch prep is tiny. The exFAT descriptor path is small but visible. The
recent atomic publish work is the right correctness tradeoff, but launch prep
should keep these writes off the last possible UI-critical moment when it can.

### Launch Handoff

Commands:

```bash
scripts/profile-launch-handoff.sh PERFREVIEW-20260629-HANDOFF-SUCCESS --replace-label --mode success --iterations 3 --delay-ms 750
scripts/profile-launch-handoff.sh PERFREVIEW-20260629-HANDOFF-SLOWFAIL --replace-label --mode slow-fail --iterations 3 --delay-ms 750
```

Key results:

- Success mode:
  - Iteration max frame gaps: about `17003us`, `17981us`, `108123us`.
  - Handoff complete: about `783-817ms`.
- Slow-fail mode:
  - Iteration max frame gaps: about `18858us`, `17986us`, `100517us`.
  - Recovery: about `2095-2162us`.

Interpretation:

The handoff success/failure outcomes are good, but the repeated third-iteration
100ms max frame gap is the clearest measured production jank target.

### Library I/O

Command:

```bash
scripts/profile-library-io.sh PERFREVIEW-20260629-LIBIO --replace-label --sample-limit 180
```

Key results:

- Launcher was suspended for the isolated DB path.
- Walk: `23.36s`, candidates `7527`.
- Classify total: `24.06s`, discoveries `9362`.
- Import/build:
  - Precompute catalog: `3.06s`, rows `7259`.
  - Metadata load: `1.15s`.
  - Insert games: `1.67s`.
  - Materialize arcade UI: `1.23s`.
- SQLite publish: `6.86MB`, copy `547ms`, total `552ms`.

Interpretation:

The exFAT DB publish is not the first-scan bottleneck. Scan/classify and RAM
catalog construction dominate, while the tmpfs-to-exFAT publish is acceptable.

### First Scan Production Path

Command:

```bash
scripts/profile-first-scan.sh PERFREVIEW-20260629-FIRSTSCAN --skip-build --replace-label --timeout 240 --thread-sample
```

Key results:

- The script failed its RAM-catalog gate:
  - `library_ready=44918ms > 41000ms`.
- Library scan complete: `40634ms`.
- Scan/classify internals:
  - `scan_us=39699862`
  - `discover_us=37118065`
  - `classify_us=39698220`
- RAM catalog construction: `4054634us`.
- Projection: `218586us`.
- SQLite publish: `6.86MB`, copy `590ms`, total `603ms`.
- DB saved: `53011ms`.
- Thread samples:
  - library/catalog work mostly CPU0.
  - UI/vsync mostly CPU1.

Interpretation:

This is the main cold-path miss. Thread placement looks broadly correct, so the
next gains are in scanning/classification work, RAM catalog construction, and
deferring durable/background work away from first interaction.

### Media Cold Boot

Command:

```bash
scripts/profile-media-cold-boot.sh PERFREVIEW-20260629-MEDIA --skip-build --replace-label --timeout 420 --keep-catalog --thread-sample
```

Key results:

- Catalog was kept; asset dir was reset.
- UI rows appeared for Arcade, Neo Geo, and Saturn.
- Arcade media finished at about `22.96s`.
- Neo Geo media finished at about `25.58s`.
- Saturn media finished at about `29.81s`.
- Phases included download, verify, save, sync, rename, parent-sync, done.
- Media thread was mostly CPU0; UI process mostly CPU1.

Interpretation:

Media cold boot is acceptable as background work, but these are long enough SD
activities that interaction-aware pausing and publish deferral are worthwhile.

### Screenshot Save And Download

Commands:

```bash
scripts/profile-screenshot-save.sh PERFREVIEW-20260629-SAVE-ARCADE --system arcade --iterations 3 --replace-label
scripts/profile-screenshot-save.sh PERFREVIEW-20260629-SAVE-NEOGEO --system neogeo --iterations 3 --replace-label
scripts/profile-screenshot-save.sh PERFREVIEW-20260629-SAVE-SATURN --system saturn --iterations 3 --replace-label
scripts/profile-screenshot-download.sh PERFREVIEW-20260629-DOWNLOAD --system all --iterations 1 --save-strategy staged --replace-label
```

Save results:

- Arcade, `24.53MB`: `2790ms`, `1894ms`, `1977ms`.
- Neo Geo, `4.97MB`: `556ms`, `371ms`, `392ms`.
- Saturn, `9.07MB`: `967ms`, `700ms`, `744ms`.

Download plus staged save results:

- Arcade: download `6335ms`, save `3364ms`, verify `1299ms`, total `11003ms`.
- Neo Geo: download `1543ms`, save `418ms`, verify `278ms`, total `2244ms`.
- Saturn: download `2520ms`, save `886ms`, verify `495ms`, total `3905ms`.

Interpretation:

Pack copy time is material. The staged strategy is the right shape, but runtime
policy should keep publish work out of active interaction windows.

## Prioritized Optimization Plan

### P0: Make Launch Handoff Frame-Safe

Goal: remove the repeated 100ms handoff frame gaps.

Actions:

1. Replace per-frame shell probing with a throttled probe or Main-provided
   status signal.
2. Add `LaunchHandoff` runtime thread policy.
3. Add a benchmark gate for `max_frame_gap_us`.
4. Re-run success and slow-fail profiles for at least 10 iterations.

### P1: Budget Catalog Worker UI-Thread Work

Goal: prevent catalog work from creating visible 100ms frames.

Actions:

1. Add per-message and per-drain trace fields for catalog worker processing.
2. Cap catalog worker drain time per frame.
3. Defer DB/projection durability until idle after RAM catalog ready.
4. Re-run first-preview and first-scan profiles.

### P1: Add Present Attribution

Goal: make the direct preview production path first-class in traces.

Actions:

1. Add `direct_preview_present_us`.
2. Add copied pixels/bytes for cached, list, and preview presents.
3. Update preview/arcade profile summaries and HTML reports.

### P1: Tighten Media And Archive I/O Policy

Goal: preserve UI smoothness when SD-card work is active.

Actions:

1. Defer full preview archive promotion until idle.
2. Keep selected preview index reads fast and bounded.
3. Pause or delay media publish if interaction starts.
4. Add active-media counters to frame summaries.

### P2: Reduce First-Scan Time

Goal: restore comfortable headroom under the 41s RAM-ready gate.

Actions:

1. Profile scan/classify at finer granularity by path type and extension.
2. Keep helper/media directories excluded.
3. Look for repeated path/string allocation in classify and RAM catalog build.
4. Preserve tmpfs SQLite build and large sequential exFAT publish.

### P2: Copy-Shape Experiments For Arcade Scroll

Goal: trim already-healthy render cost without risking correctness.

Actions:

1. Benchmark dense list copy versus current segmented copy.
2. Compare wrapped and non-wrapped circular-surface cases.
3. Gate on p99 work, p99 present, copied bytes, and visual correctness.

### P2: Benchmark Guard Rails

Goal: make future performance conclusions harder to misread.

Actions:

1. Make all scripts emit run context, binary scope, feature scope, and deployment
   verification.
2. Make first-preview and launch-prep fail on gate failures.
3. Keep visual captures outside timed sections by default.
4. Align acceptance defaults with documented production catalog policy.

## Bottom Line

The production render path is in good shape. Steady Arcade and preview browsing
have milliseconds of headroom on the dual-core Cortex-A9, and thread samples
show that the current CPU0/CPU1 split is mostly doing what it should.

The next deep wins are orchestration wins: remove shell/proc polling from the
handoff hot period, budget catalog worker work, make direct preview present
fully attributable, and keep exFAT media/archive work away from active
interaction. The SD card is not killing steady scroll; it is showing up in cold
scan, media publish, descriptor publish, and background work that can still land
on visible frames.
