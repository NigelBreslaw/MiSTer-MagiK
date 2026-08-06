# ARMv7 profiling evidence, 2026-08-06

## Outcome

MiSTer's Linux 4.19 Cortex-A9 PMUv1 path is usable for bounded application
profiling. Arm Performix is not compatible with this ARMv7 target, while an
audited ARMv7 hard-float `gatord` can produce a bounded Streamline capture.

The first PMU-guided optimization removed repeated dual-slot manifest loading
from persisted search. One immutable, validated search snapshot is now reused
for the lifetime of a launcher catalog version and replaced when that version
changes. The benchmark uses the same snapshot path as the launcher.

## Device compatibility

The installed device exposed the `armv7_cortex_a9` perf event source and
`perf_event_paranoid=2`. The successful group uses the calling thread on any
CPU, counts user and kernel execution, and reads ordered values without
multiplexing. The PMUv1/kernel ABI boundaries observed by the diagnostic were:

- `exclude_kernel` event opens fail with `EINVAL`;
- `PERF_EVENT_IOC_ID` fails with `ENOTTY`; and
- ordered grouped reads succeed and report cycles, instructions, Cortex-A9 L1D
  accesses/refills, branches, and branch mispredicts.

The application therefore falls back only for the explicit legacy ABI errors
`EINVAL`, `ENOTTY`, and `EOPNOTSUPP`. Other failures remain fatal.

## PMU baseline

Three independent `scripts/agent benchmark pmu-profile` runs passed on revision
`8e83b333`:

| Capture | warm prepare p50 | warm total p50 | manifest spans |
| --- | ---: | ---: | ---: |
| `1786047952` | 9,509 us | 17,296 us | 88 |
| `1786048063` | 9,434 us | 17,190 us | 88 |
| `1786048088` | 9,405 us | 17,146 us | 88 |

Across the 264 `search.manifest` samples, manifest loading consumed 6,939,659
to 8,013,660 cycles per query, with means of 7,500,680 cycles, 6,250,967
instructions, 21,058 L1D refills, and 118,645 branch mispredicts. This made
manifest validation—not query preparation—the novel optimization target.

## Screensaver PMU data

All three baseline and all three post-change PMU suites also recorded 180
samples for each screensaver phase: 720 attempted spans and zero dropped spans
per capture. The three-run baseline aggregate was:

| Phase | samples | mean cycles | mean instructions | mean L1D refills | mean branch mispredicts |
| --- | ---: | ---: | ---: | ---: | ---: |
| background | 540 | 1,399,126 | 1,839,167 | 532 | 786 |
| advance | 540 | 60,558 | 25,777 | 215 | 251 |
| draw-order | 540 | 148,168 | 104,516 | 656 | 1,487 |
| tile-blit | 540 | 4,041,828 | 1,301,343 | 41,607 | 6,796 |

The corresponding post-change mean cycle counts were 1,402,741, 60,370,
148,770, and 4,004,900. This confirms that the search-only change did not move
the screensaver phase distribution materially. It also makes `tile-blit` the
clear future screensaver target, but no speculative raster change was included
in this work because the measured manifest reuse supplied a larger, isolated
optimization with an existing freshness key.

## Optimized result

Revision `5049d0a7` was delivered as a clean committed Dev runtime. Three
independent post-change PMU suites passed:

| Capture | warm prepare p50 | warm total p50 | manifest spans |
| --- | ---: | ---: | ---: |
| `1786050522` | 35 us | 7,351 us | 0 |
| `1786050555` | 34 us | 7,216 us | 0 |
| `1786050601` | 35 us | 7,385 us | 0 |

The run-to-run ranges changed from 9,405–9,509 us to 34–35 us for warm Rust
preparation, and from 17,146–17,296 us to 7,216–7,385 us for total warm search.
Using the mean of the three per-run p50 values, preparation fell 99.6% and total
search fell 57.5%. All 264 repeated manifest spans disappeared. SQLite is now
the dominant measured search phase, so further work should target its FTS query
and result materialization rather than more manifest caching.

The normal `scripts/agent benchmark search` gate also passed in capture
`1786050627`; its launcher UI verification returned 221 results for `A` in 695
ms. `scripts/agent benchmark screensaver` passed in capture `1786050643`; both
its authoritative cadence and attribution passes reported zero physical dropped
frames and zero latch drops.

## Streamline capture

The audited source was Arm gator 9.7.2 at commit
`f0774012f36dbdb543e082d3e14ca9db20d0432d`. The temporary ARMv7 hard-float
daemon had SHA-256
`2d38e36368addc77e8abc7c0c21bd7d88302de6afa243201d931b1a51962b346`.
The pinned source required two generated-build-only workarounds:

- replace the generated `-I daemon` with `-idirafter daemon` so the daemon's
  own `time.h` does not shadow the C library header; and
- define `libgpuinfo=libarmgpuinfo` to match the pinned submodule namespace.

No file in the read-only reference checkout was modified. These workarounds are
evidence about the pinned upstream source, not repository build policy.

`MISTER_GATORD_PATH=/absolute/path/to/gatord scripts/agent benchmark streamline`
passed in capture `1786050032`. It recorded the fixed `pmu-profile screensaver`
workload at low sample rate for at most ten seconds, with kernel execution
included and call-stack unwinding disabled. The retrieved 713,808-byte archive
had SHA-256
`47cbc3384e3a68d0d14bc497256ac03ffc47bef2afa0845ae620d3b562e8e777`.
The ignored local evidence contains both `mister-magik.apc` and its verified
archive; neither binary capture nor the temporary daemon is committed.
