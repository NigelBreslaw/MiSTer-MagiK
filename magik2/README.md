# MiSTer MagiK Tooling 2.0

This is a separately owned replacement for the development loop. It does not
modify the production MiSTer MagiK application, Main, or FPGA platform.

The public entrypoint is `scripts/magik2`:

```text
scripts/magik2 deploy
scripts/magik2 check smoke
scripts/magik2 check motion --profile
scripts/magik2 watch
scripts/magik2 status
scripts/magik2 stop
```

The host uses `MISTER_IP`; it caches the native token locally with mode 0600.
The native service defaults to TCP port 7500. Existing SSH credentials are used
only by the fixed-purpose bootstrap/repair adapter when native discovery is
unavailable or lacks a required capability.

## Current implementation

The independent agent and probe are operational. `deploy` fingerprints relevant
probe inputs, builds only when they change, streams a hash-checked replacement,
and waits for the first RGB565 presentation. A healthy unchanged deployment is
a no-op. The agent owns the probe across host disconnects and agent upgrades;
`stop` explicitly resumes Main's ordinary launcher.

`watch` is a localhost-only browser viewer. The probe produces its own metrics,
logs, and already-rendered RGB565 previews; the agent retains only the newest
preview and a bounded log tail, so a slow or disconnected viewer cannot block
rendering or control traffic.

`check` uses a fresh native Slint system-testing session and restores ordinary
persistent probe mode afterwards. The motion profile is the same workload,
sampled at 99 Hz by the pinned ARM-compatible pprof revision; its folded stacks
and flamegraph are retrieved into the result bundle. Running those scenarios
requires authenticated access to the pinned `slint-testing==0.3` private index.
Without that access, `check` reports the missing client rather than falling back
to a legacy bridge or claiming a pass.

Result directories under `build/magik2-results/` contain source revision and
dirty-state context, incremental events, phase timings, retained screenshots,
and any profile artifacts. They are local evidence, not a workflow database.

The wire contract is deliberately small. JSON headers are capped at 64 KiB;
bulk bytes follow a fixed 64-bit big-endian length and are never encoded into a
header. Optional response fields are ignored by the host.
