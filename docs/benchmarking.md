# Benchmarking policy

`scripts/agent benchmark` is the only agent-facing performance workflow. It is
flag-free and selects one canonical scenario from the current HEAD commit's
changed components. It requires an exact clean commit and an artifact receipt
for that commit.

## Velocity scenarios

Launcher and framebuffer changes use:

```text
Select → QualifyBuild → PrepareDevice → Warmup
→ Capture → Analyze → Evaluate → Restore
```

The fixed gates are average FPS ≥ 55, p99 work ≤ 14,500 µs, p99 wall ≤
16,000 µs, maximum wall ≤ 16,667 µs, and zero presentation errors. Scenario,
trace paths, warmup, duration, and thresholds are policy—not command flags.
Velocity scenarios, never row jumps, support Arcade performance conclusions.

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
