# Deep profiling baseline, 2026-08-12

## Outcome

The first MiSTer MagiK deep-profiling campaign produced decision-grade GUI,
system-entry, observer, and device-agent I/O attribution. The two final GUI
runs were independently restarted and retained separate unprofiled control,
PMU, and system-wide Streamline arms. Their PMU totals agreed within 1.1%.

This is attribution evidence, not an optimization change. The unprofiled arms
remain latency authority and protocol-v5 counters remain cadence authority.
The campaign did not pass every product or artifact gate:

- both GUI controls recorded 10 physical repeated vblanks, with zero latch
  drops, sequence gaps, or ownership losses;
- transition cadence failed, and the final capture correctly failed artifact
  validity because three reverse portrait Settings legs had insufficient frame
  telemetry;
- launch/return failed twice in its control arm because no new return capsule
  appeared within the bounded 20-second wait; and
- the retired `library.sqlite3` snapshot operation was attempted twice and
  recorded as not applicable on the exact Catalog V3 layout.

System entry and observer attribution passed both artifact and product gates.
Agent I/O passed artifact validity and is attribution-only by definition.

## Provenance

The canonical delivery workflow reconciled a clean commit before the final
campaign and required no device changes. The installed runtime identity was:

| Field | Value |
| --- | --- |
| installed MagiK revision | `17ed85282b10099cddd6eda4a66197bdc79513c0` |
| boot ID | `100e18c7-f8c4-41a6-91fa-c7fd14df0b13` |
| platform manifest SHA-256 | `081bc7224327a421ddb24812375ce052329c90d04c9a22b5c99ce0e3901bba9c` |
| GUI SHA-256 | `10c9be8e3a0b2c83e1f4abfede9f2010fd938840fb6573cebc7e52a049368a1b` |
| agent build | `agent-v16` |
| agent SHA-256 | `148d7df8e41b90302b7e3368b483b0d042c7ddc68d1dc60a707686b351f917a7` |
| gatord | `Streamline Data Recorder v9.7.2 (Build oss)` |
| gatord SHA-256 | `2d38e36368addc77e8abc7c0c21bd7d88302de6afa243201d931b1a51962b346` |
| capture clock | device `CLOCK_MONOTONIC` |
| display | HDMI RGB565, 1280x720p60 |

Retaining agent function symbols increased the optimized installed agent from
1,125,588 to 1,518,492 bytes: 392,904 bytes, or 34.9%. Streamline resolved the
stable application symbols in the exact installed 1,518,492-byte image. The
symbol-policy change did not alter steady-state ownership or launcher
protocol behavior; agent-v16 identifies the additive profiling evidence.

## Curated evidence index

Raw evidence remains ignored. These paths are local provenance pointers, not
committed artifacts.

| Scenario | Capture | Artifact | Product | Streamline archive SHA-256 |
| --- | --- | --- | --- | --- |
| GUI baseline 1 | `build/agent-benchmarks/gui-frame-attribution/1786526893` | passed | failed | `f0b7ff1b92cb23967f1d768e94df7eb3e7c1a4c2b2a35295d9783421475061fc` |
| GUI baseline 2 | `build/agent-benchmarks/gui-frame-attribution/1786526942` | passed | failed | `c84d469f383f8bf33ab077fa3042aa47ed049d1b1f7016b45a097765591c9823` |
| system entry | `build/agent-benchmarks/system-entry-critical-streamline/1786526993` | passed | passed | `15bf8e4b164553797a43d81632ebe4072304b384f69e8c9708bd20d3e4c745ed` |
| transitions | `build/agent-benchmarks/transition-streamline/1786527547` | failed | failed | `0a0c1339d5ce28d44e8892132234fd15c0f1dc99f9fc891eaac64ea0bb9795f8` |
| observer | `build/agent-benchmarks/agent-observer-attribution/1786526732` | passed | passed | `ec1c110f2090d753f9f1ae367be1ddd2d3ccbcaa042fdcb5299de80bc2a77cd2` |
| agent I/O | `build/agent-benchmarks/agent-io-attribution/1786526693` | passed | attribution-only | `0c74a8de6d0203a76ce3a1af32eb08b2a80b5cb06d610b5786a4f7c2929d1c52` |
| launch/return attempts | `build/agent-benchmarks/launch-return-attribution/1786525606` and `1786527226` | failed | not evaluated | no Streamline arm reached |

The launch/return failures occurred before the v1 top-level failure-retention
fix. Their control `summary.json`, event log, launcher log, and Main status are
retained. Future failures emit `mister-magik-launch-return-attribution-v1`
with `artifact_status=failed` without replaying later mutation arms.

## GUI frame attribution

The final PMU runs recorded 603,753,004 and 597,289,434 cycles. They reported
4.54 and 4.36 million cycles per measured frame, IPC of 0.523 and 0.536, and
L1D refill ratios of 13.7% and 13.3%.

The first run's largest non-overlapping named phase was navigation-transition
raster at 174,479,331 cycles, 28.9% of total route cycles, with IPC 0.440 and
an 18.4% L1D refill ratio. The enclosing custom-layer-generation span measured
227,725,904 cycles and must not be added to its child phases. Base damage copy,
Arcade overlay copy, catch-up restoration, and preview overlay copy accounted
for 74.5 million named cycles in aggregate. The unprofiled control copied
10.10 bytes for every byte of Slint damage: 7.12x invalidation plus 2.98x
catch-up amplification.

Both controls repeated the same cadence result: 10 physical repeated vblanks,
zero latch drops, zero ownership losses, and zero sequence gaps. PMU added
16.6 ms to the first full route, which is why it is not cadence authority.

## Other workloads

- C64 system entry reached the first list frame in 43 ms and complete correct
  presentation in 56 ms. Its NavPack open was 351 us and CPU1 adoption was
  2.426 ms.
- SNES reached the first list frame in 43 ms and complete presentation in
  86 ms. Its NavPack open was 369 us, CPU1 adoption was 3.225 ms, first-frame
  preparation was 8.648 ms, and preview work was 4.560 ms after 26.068 ms of
  request age.
- The final orientation fade windows peaked at 25.834-35.640 ms of whole-frame
  work. Zoom peaked at 28.623-37.302 ms. Zoom's first normal-to-portrait leg
  recorded 13 repeated vblanks and one ownership loss. Latch drops and sequence
  gaps remained zero.
- Adaptive and full framebuffer observers added 89.808 ms and 101.214 ms to
  the fixed Home-pan route. Neither introduced a latch drop.
- At 100 ms cadence, telemetry process discovery consumed 1.435-1.721 seconds
  across 25-30 samples. The fixed GUI route did not regress in these noisy
  short controls, so this is a system-cost target rather than a proven cadence
  regression.
- Static PNG capture took 146.004-163.548 ms. High-entropy PNG took
  188.221-197.256 ms; RGB conversion and zlib together consumed 140-150 ms.
  The peak simultaneous buffer ownership was 5,811,573 bytes.
- The first V1 listing of 961 Arcade entries took 1.449 seconds, of which
  1.411 seconds was enumeration. Its repeat took 84.955 ms. V2 took
  49.550-50.744 ms, dominated by 25 ms enumeration and 11-13 ms sorting.

## Restoration

After the campaign, typed device checks reported `MiSTer_MagiKDev`, launcher
state `LauncherActive`, the normal Home screen, RGB565 1280x720, the configured
settings and manifest, and `arming=clear`. Main and the launcher were healthy,
the latch backend was active, and no boot-loop arming path remained.

The ranked recommendations are recorded separately in
`history/2026-08-12-deep-profiling-optimization-shortlist.md`. No optimization
was implemented in this campaign.
