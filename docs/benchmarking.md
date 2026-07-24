# Benchmarking policy

`scripts/agent benchmark` is the only agent-facing performance workflow. It is
flag-free and selects one canonical scenario from the current HEAD commit's
changed components. It requires an exact clean commit and an artifact receipt
for that commit. Before deploying the temporary runtime, it verifies the
installed development manifest and every bound Main, scanout, and latch
artifact; an incoherent platform is not measured.

## Velocity scenarios

Launcher and framebuffer changes use:

```text
Select → QualifyBuild → PrepareDevice → Warmup
→ Capture → Analyze → Evaluate → Restore
```

The fixed gates are average FPS ≥ 55, p99 work ≤ 14,500 µs, p99 wall no more
than the measured refresh period plus 500 µs, and maximum wall below 1.5
measured refresh periods. Presentation errors, latch drops, and vsync misses
must all remain zero. Scenario, trace paths, warmup, duration, and thresholds
are policy—not command flags. Velocity scenarios, never row jumps, support
Arcade performance conclusions.

Screensaver renderer changes select the fixed screensaver-velocity scenario at
1280x720 output and framebuffer resolution. It selects the 720p boot mode
through the typed INI mutator and performs one bounded normal Linux reboot,
independent of MagiK Main's command channel. The freshly
booted launcher starts on Home,
uses production controller input to open Settings and activate Show
Screensaver, then traces 30 seconds from activation. The normal catalog
lifecycle remains enabled to reproduce a real boot. The scenario reports the
full active interval, the first 180 active frames, and the remaining steady
frames. The device remains in 1280x720 afterward; only the temporary benchmark
launcher environment is cleared. The standard velocity gates apply to the
overall and startup intervals; steady results are diagnostic.

## Cold and catalog scenarios

Catalog, preview/media, and library-persistence changes use:

```text
Inspect → SnapshotData → EstablishFixture → Execute
→ CollectEvents → Evaluate → Restore
```

The device runtime, Catalog V3 directory, library database, and durable Arcade
bootstrap index are explicitly snapshotted before mutation. Evaluation consumes
structured device events. Restoration is unconditional after the first mutation;
failed compensation produces `recovery_required`.

The canonical scenarios own first scan, cold preview/media preparation, preview
index refresh, catalog lifecycle/contention, launch preparation, pack access,
and library persistence. Labels, duration matrices, skip-build modes, history
appenders, environment files, and shell/AWK parsers are retired.

## Evidence

Progress records phase changes and ten-second heartbeats. Benchmark evidence is
tied to the exact Git SHA and kept separately from attended release evidence.
Pure offline report generators under `scripts/bench/` may analyze existing data;
they must not contact or mutate a MiSTer.
