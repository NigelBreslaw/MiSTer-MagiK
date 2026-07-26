# Benchmarking policy

`scripts/agent benchmark [SCENARIO]` is the only agent-facing performance
workflow. Scenarios are a closed typed registry rather than a flag matrix. It
never builds, deploys, replaces platform files, or reboots the MiSTer. The
installed platform manifest and its hashes are the benchmark identity, and its
delivery reconciliation against the clean local Git HEAD must be a no-op.
Host-only benchmark tooling changes therefore do not force an identical runtime
revision, while pending runtime or platform changes remain a hard failure.

Supported scenarios:

- `screensaver` (the default)
- `catalog-lifecycle`

New benchmarks must add a named registry entry and a fixed typed device
request. They may not expose arbitrary commands, duration knobs, remote paths,
or generic environment overrides.

## Persisted search

The default screensaver workflow begins with a short, read-only benchmark of
the active `arcade` system shard. Four representative queries each record a
first result, one warm-up, and 20 measured iterations. Evidence separates Rust
query preparation, SQLite FTS5 execution, Rust result finalization, and total
latency, with warm p50, p95, and maximum timings. The search phase is
informational; the screensaver profile remains the correctness gate.

## Catalog lifecycle

```text
Verify installed platform, health, and exact clean revision
-> suspend the ordinary launcher through Main
-> create a fixed isolated /tmp catalog root
-> build the full catalog from installed read-only media and core inputs
-> inspect the registry and every system shard
-> record elapsed time and per-system game counts
-> remove the isolated fixture
-> resume the ordinary launcher
-> verify platform identity and health
```

The scenario redirects the sharded catalog, library database, arcade bootstrap
index, ready snapshot, and builder/refresh locks beneath
`/tmp/mister-magik/catalog-lifecycle-benchmark`. Production catalog and library
artifacts are never renamed, deleted, or overwritten. Cleanup and launcher
resume run after every post-suspend success or failure.

Evidence is written under
`build/agent-benchmarks/catalog-lifecycle/<timestamp>/` as the refresh log,
catalog inspection TSV, structured summary, and Markdown report.

## Installed screensaver profile

The fixed workflow is:

```text
Verify installed platform and health
-> require screensaver-pprof-v1 and a cached catalog
-> transactionally select 1280x720 HDMI and verify a 1280x720 RGB565 framebuffer
-> start the ordinary launcher with a one-shot environment
-> navigate Home -> Settings -> Show Screensaver
-> profile and stream telemetry for 45 seconds
-> restore the ordinary launcher
-> retain the confirmed 1280x720 HDMI mode
-> verify platform identity and health
```

The benchmark confirms `hdmi-1280x720p60` and intentionally leaves that mode
active after the profile. It records the original mode and INI hash, then
uses the confirmed 720p INI as the final-state baseline. Launcher/profile
cleanup remains mandatory and independent of the retained display mode.

The one-shot launcher environment sets `MISTER_CATALOG_REFRESH=off`. The
launcher must report both the disabled refresh policy and an inactive catalog
worker throughout each active profile. The environment removes itself when the
launcher consumes it; bounded host cleanup is still unconditional after
success, failure, interruption, or an incomplete second run.

Each run uses the production composition and latch-backed presentation path.
Navigation frames are excluded. The first three frames for which runtime
telemetry reports `screensaver_active=true` are activation warm-up: their
timings are recorded as startup evidence but never fail the benchmark. A
screensaver may take several frames to become visible without creating a
user-visible defect.

Steady state begins on the fourth active screensaver frame. The benchmark uses
the median nonzero measured refresh period and completion timestamps to count
the physical refresh intervals in that window. Wrapping latch flip-counter
deltas count unique presentations. Unique presentation FPS must remain within
0.1 FPS of the measured refresh rate, with no repeated refresh, completion gap
over 1.5 refresh periods, incomplete latch, sequence mismatch, non-unit flip,
presentation error, latch drop, or final pacing miss. Submitted FPS, wall-time
overruns, P99, and maximum timings remain diagnostic evidence; contiguous
submitted sequences alone do not prove unique physical refreshes.

This distinction is intentional. Do not tighten startup timing because of a
slow first render, asset loading, allocation, profiler startup, or other
one-time activation work. The benchmark exists to prove that an already
running screensaver does not drop frames.

The complete 45-second run remains the correctness gate. Performance comparisons
also report the final 15 seconds separately, after the parade has had roughly
30 seconds to reach its populated state.

## Restoration

Restoration removes the launcher environment, frame-analytics lease, and
temporary remote profile files, then restarts the ordinary launcher. The
workflow fails separately when performance is outside its gates or restoration
cannot prove a clean, healthy device. The confirmed 720p mode and its exact
`MiSTer.ini` contents, device boot ID, and installed manifest must be unchanged
after profiling.

## Evidence

The SVG flamegraph, folded stacks, profile metadata, telemetry stream,
`summary.json`, and `report.md` are written under
`build/agent-benchmarks/screensaver/<timestamp>/`. Evidence records the
installed revision, display route and framebuffer geometry, and GUI, Main,
scanout-module, and latch-RBF hashes. The report includes presentation
continuity, timing outliers, CPU phases, periodic timing, one-second
maintenance cohorts, and raster-position holds. Pure offline report generators under
`scripts/bench/` may analyze existing data; they must not contact or mutate a
MiSTer.
