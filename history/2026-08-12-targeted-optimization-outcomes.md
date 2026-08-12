# Targeted optimization outcomes, 2026-08-12

## Outcome

The targeted campaign used the existing deep-profiling campaign as its before
data. It did not repeat the broad baseline. Each runtime candidate was
committed, delivered as an exact clean revision, and measured with only its
own fixed benchmark. PMU and pprof results were used for attribution;
unprofiled routes remained performance authority.

Five optimizations were accepted: navigation raster, orientation zoom,
RGB565 half-scale snapshots, streaming PNG capture, and telemetry process
discovery. Fade, latch-slot coherency, directory sorting, and system-entry
raster experiments were closed with additive reverts after failing their
gates. The system-entry phase split remains as dormant benchmark evidence.

Raw artifacts remain ignored under `build/agent-benchmarks/`. The paths below
are local provenance pointers and are not committed evidence.

## Decisions

| Candidate | Commits | Decision | Target-phase result | End-to-end or product result | Evidence |
| --- | --- | --- | --- | --- | --- |
| Transition phase attribution | `092ccef6` | retained | Identified Settings blits and zoom overwrite as actionable; replaced empty orientation measurements with kernel evidence | Dormant outside the fixed routes | `settings-navigation-pprof/1786532514`, `orientation-transition-fade-pprof/1786532576`, `orientation-transition-zoom-pprof/1786532646` |
| Navigation raster and capture cadence | `3809fc16`, repair `3e2ef67b`, carrier `20a57da3`, boundary/evidence repairs `b4305f8e`, `b379ea1d`, `5922d66a`, `e37ab0c9` | accepted | Settings-blit P95 fell 7.201 to 4.761 ms in landscape (-34%) and 5.988 to 3.820 ms in portrait (-36%); the source carrier removed the 32 ms controlled-capture frame from the animation window | All 12 landscape and portrait legs now have zero physical repeated vblanks, latch drops, sequence gaps, and ownership loss; animation work max is 3.1-5.5 ms | `settings-navigation/1786544530`; attribution `settings-navigation-pprof/1786543239` |
| Orientation fade | `663a5ea7`, repair `2ed8d7bd`, revert `d7683db9` | closed | Initial change regressed; row-locality repair improved the kernel by only about 1% | Below gate. Revert was delivered; the operator explicitly waived another fade check because fade is unused | `orientation-transition-fade/1786533592`, `orientation-transition-fade/1786534049`, `orientation-transition-fade-pprof/1786533659`, `orientation-transition-fade-pprof/1786534111` |
| Orientation zoom | `43473520`, row-locality repair `ec227344`, exact evidence `e50c5bdb`, build repair `f6d6f175`, source carrier `997981ec`, redraw repair `254a60c2` | accepted | Kernel P95 fell 4.447 to 3.423 ms (-23%); aggregate kernel time fell 18.8%. Moving setup onto an unchanged confirmed source carrier reduced animation-frame work max from 26.5-32.5 to 7.6-9.1 ms (-68% to -77%) | All six directed legs now sustain about 60 fps with zero physical repeated vblanks, latch drops, sequence gaps, and ownership loss | `orientation-transition-zoom/1786545800`; attribution `orientation-transition-zoom-pprof/1786535035` |
| Latch-slot coherency | `94b69aaa`, revert `c926df9b` | closed | No material reduction from the baseline 16 full copies, 10.10x amplification, or 74.5M named copy cycles | No qualified product improvement; reverted | `gui-frame-attribution/1786535604` |
| RGB565 half-scale snapshots | `930ac4f5` | accepted | NEON half-scale met its isolated P95 gate | Adaptive observer route overhead fell 89.808 to 76.506 ms (-14.8%); full mode fell 101.214 to 92.243 ms (-8.9%) and did not regress | `agent-observer-attribution/1786536416` |
| Streaming PNG capture | `370c751d`, identity repair `1b58ac11` | accepted | RGB conversion was roughly halved; high-entropy total fell from 188.221-197.256 to 149.107-159.908 ms (-20% range) | Static total fell from 146.004-163.548 to 106.670-121.026 ms; peak simultaneous ownership fell 5.81 to 2.21 MB (-62%) | `agent-io-attribution/1786537327` |
| Directory natural sort | `194592af`, repair `293f992d`, revert `a7b8811e` | closed | V2 sort fell from 11.412-12.599 to 4.091-4.600 ms after repair, clearing the phase gate | V2 total was 47.884-58.530 ms versus 49.550-50.744 ms baseline, so the required 10% end-to-end improvement was absent; reverted. V1 remains for desktop fallback and compatibility | `agent-io-attribution/1786537709`, `agent-io-attribution/1786538008` |
| Telemetry discovery | `d51c4b0b`, release repair `2045e970` | accepted | 100 ms arms fell from 48-57 ms discovery per sample to about 5.7 ms (-88% to -90%) | Aggregate sample wall cost fell about 48%-72%; fixed Home pan showed no regression and all arms retained zero latch drops | `agent-observer-attribution/1786538700` |
| System-entry phase attribution | `fc8eef15` | retained | C64: Slint 14.176 ms, list 8.637 ms, latch 3.270 ms. SNES: Slint 13.475 ms, list 9.294 ms, overlay 6.033 ms, latch 2.914 ms | Evidence is benchmark-owned and dormant in production | `system-entry-critical-profile/1786539529` |
| System-entry raster | `2ade4c65`, repair `05885551`, revert `ead0f747` | closed | The first attempt lowered SNES first-list preparation by 1.212 ms (-14%); the repair removed native chrome overdraw and lowered the quick preparation result to 7.097 ms | Ten-process confirmation remained C64 56 ms P95 and SNES 90 ms P95, unchanged from the first attempt and not better than the canonical 56/86 ms baseline. Both changes were reverted. Post-revert route passed at C64 55 ms and SNES 88 ms | `system-entry-critical/1786540151`, `system-entry-critical-confirm/1786540205`, `system-entry-critical/1786540668`, `system-entry-critical-confirm/1786540701`, post-revert `system-entry-critical/1786541146` |

## Installed identities

Representative retained measurements were bound to the following exact
installed identities:

| Candidate | MagiK revision | GUI SHA-256 prefix | Agent identity |
| --- | --- | --- | --- |
| Navigation cadence | `e37ab0c9` | `bbdda1936bed` | unchanged campaign agent |
| Zoom cadence and final state | `254a60c2` | `aff2a845dbc3` | agent-v21 |
| Half-scale | `930ac4f5` | `129048509ad7` | agent-v16, `148d7df8e41b` |
| PNG | `1b58ac11` | `c71a51bb2218` | agent-v17, `77c9ba934638` |
| Telemetry | `2045e970` | `a44bf4766db6` | agent-v21, `be5d42d12156` |

The final typed status checks reported `MiSTer_MagiKDev`, launcher state
`LauncherActive`, the normal Home screen, RGB565 1280x720, protocol v5, and
`arming=clear`.

## Remaining opportunities

1. Adaptive framebuffer observation remains expensive at 76.506 ms of route
   overhead. Streamline already attributes the surrounding system activity;
   profile only the next proposed producer/transport change.
2. PNG still spends roughly 75-76 ms in zlib on high-entropy frames. This is a
   viable lower-priority target if operator capture latency matters.
3. Directory sorting is no longer the end-to-end limiter. Enumeration and
   serialization dominate V2, while V1 metadata behavior should remain a
   compatibility concern rather than an optimization target.
4. System-entry first-frame publication is now separated correctly, but the
   native-background experiment proved that reducing one CPU phase did not
   move complete readiness. The evidence points to preview/overlay and
   confirmation timing, not another Slint-background rewrite.

The original before values and campaign provenance remain in
`history/2026-08-12-deep-profiling-baseline.md`.
