# Fast five-system pprof profile

Date: 2026-08-26

Branch: `nigel/arcade-catalog-prototype`

## Run

One symbolized 997 Hz pprof session covered snapshot read and validation plus
publication of every system currently supported by the new prototype: Amiga,
Arcade, C64, DOS, and X68000. The run followed a verified supervised reboot,
published into an isolated root, and then passed the exact row comparator.

- Systems: 5
- Games: 18,078
- Missing, unexpected, or changed rows: 0
- pprof duration: 18.095 seconds
- Samples: 11,116 hits across 2,183 unique stacks
- Production registry changed: no

Profiled wall times are attribution evidence rather than authoritative timing:

| Component | Profiled elapsed |
| --- | ---: |
| Snapshot read, decode, and validation | 1.419 s |
| Five-system artifact publication | 16.670 s |
| Complete command before profile rendering | 19.372 s |
| C64 | 10.830 s |
| Amiga | 2.393 s |
| Arcade | 1.109 s |
| DOS | 1.013 s |
| X68000 | 0.688 s |

## Attribution

C64 consumed 64.96% of system-publication wall time. Its FTS build took 3.357
seconds: the pipelined row loop took 1.886 seconds, FTS optimization 0.597
seconds, integrity checking 0.673 seconds, and autocomplete insertion 0.166
seconds. Copying the completed C64 artifacts to exFAT took another 3.068
seconds: SQLite 2.306 seconds, navigation 0.159 seconds, and NavPack 0.603
seconds.

The folded profile's useful inclusive indicators are not additive because
stacks overlap:

| Stack family | Samples | Share |
| --- | ---: | ---: |
| JSON decode/encode | 1,179 | 10.6% |
| Artifact hashing | 856 | 7.7% |
| Sequential artifact copy | 564 | 5.1% |
| Persisted-search Rust work | 438 | 3.9% |
| NavPack | 409 | 3.7% |
| LZ4 | 234 | 2.1% |

The profile reinforces three optimization targets: replace the large JSON
interchange with a compact binary snapshot, stop emitting duplicate navigation
representations once the parallel UI reader no longer needs them, and reduce
the copied/hash-checked artifact volume. Weakening FTS optimization remains a
bad trade because the focused C64 experiment measured slower interactive
search.

## pprof limitation

6,821 samples (61.4%) contain only a thread name. A second and final run rebuilt
the statically bundled SQLite C code with frame pointers, but the ARM signal
unwinder still could not unwind those samples. They coincide with the long
SQLite phases, but pprof cannot safely assign them to individual SQLite C
functions. The explicit FTS and artifact phase telemetry above supplies the
wall-time boundary; a deeper SQLite attribution would require PMU/Streamline
or function-graph evidence rather than another pprof reboot.

## Evidence

- Profile report: `build/agent-benchmarks/fast-five-pprof/f1385b665/report.json`
- Flamegraph: `build/agent-benchmarks/fast-five-pprof/f1385b665/profile.svg`
- Folded stacks: `build/agent-benchmarks/fast-five-pprof/f1385b665/profile.folded`
- Phase log: `build/agent-benchmarks/fast-five-pprof/f1385b665/build.log`

The isolated device artifacts were removed after capture. No branch was pushed.
