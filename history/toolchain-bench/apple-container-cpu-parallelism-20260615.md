# Apple-container CPU parallelism - 2026-06-15

Goal: let the local Apple-container ARM build use all online Mac CPUs instead
of the old fixed 3-CPU limit, then measure whether it helps.

Change:

- `magik-gui/build-arm64-apple-container.sh` now defaults
  `MISTER_APPLE_CONTAINER_CPUS` from `getconf _NPROCESSORS_ONLN`, falling back
  to `sysctl -n hw.logicalcpu`, then `3`.
- The build container receives the same CPU count through `--cpus`,
  `CARGO_BUILD_JOBS`, `MAKEFLAGS`, and `CMAKE_BUILD_PARALLEL_LEVEL`.
- Build paths, FFmpeg include paths, target cache layout, and linker settings
  were intentionally left unchanged.

Benchmark setup:

- Host online CPUs: `10`.
- Baseline builder: `container builder start --cpus 3 --memory 5g`.
- All-CPU builder: `container builder start --cpus 10 --memory 5g`.
- Each scenario used one warmup and three measured samples through
  `scripts/bench-debug-build.sh`.
- Logs confirmed `==> build CPUs: 3` for baseline runs and
  `==> build CPUs: 10` for all-CPU runs.

Measured medians:

| Scenario | 3 CPUs wall | 10 CPUs wall | 3 CPUs Cargo | 10 CPUs Cargo | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| `build-ui-opts` / `noop-warm` | 4.927s | 4.945s | 1.86s | 1.87s | noise |
| `build-ui-device` / `noop-warm` | 4.901s | 4.919s | 1.84s | 1.85s | noise |
| `build-ui-opts` / `touch-rust-bin` | 20.370s | 14.336s | 17.06s | 10.93s | 29.6% wall-time faster |

Binary sizes stayed unchanged:

- `release-opts`: `5041980` bytes.
- `release-device`: `5893828` bytes.

Regression checks:

- `magik-gui/build-arm.sh --video` passed with `==> build CPUs: 10`, reused the
  existing minimal FFmpeg cache, and produced a `9554764` byte `ui,video`
  `release-device` binary.
- No logs contained `fatal error: libavutil/avutil.h`.
- No logs contained `linker 'cc' not found`.
- No logs contained `cannot find 'ld'`.
- No logs contained `expected binary not found`.

Conclusion:

- Accept the change. No-op warm builds are dominated by fixed launch/Cargo
  overhead, so more CPUs do not matter there.
- Incremental Rust edit rebuilds benefit materially: the measured
  `touch-rust-bin` `release-opts` median improved from `20.370s` to `14.336s`.
- The previous missing-header/linker failure modes did not recur because this
  change only adjusts parallelism, not build paths or linker selection.
