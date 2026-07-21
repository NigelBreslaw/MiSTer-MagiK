# UI benchmark assets

This directory contains checked-in deterministic UI fixtures consumed by Rust
benchmark scenarios. Fixture manifests and media are inputs, not executable
workflows.

Run component-selected performance qualification with:

```text
scripts/agent benchmark
```

The benchmark command owns exact-SHA build qualification, supervised launcher
operation, velocity input, structured trace collection, fixed thresholds, and
restoration. Do not add synchronization, scene, capture, or profiling scripts
for these assets. Humans may use `mister --capture-buffer` for an attended still.
