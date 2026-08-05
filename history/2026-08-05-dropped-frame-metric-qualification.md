# Dropped-frame metric qualification — 2026-08-05

## Scope and source identity

This record qualifies the repository-wide terminology and gate migration at
functional commit `de639451838e3020d71d6cdda1248cb3e3b967b2`. A dropped frame
is one physical refresh that displayed the previous frame because no new frame
was confirmed. An authoritative animation window requires exactly zero.

The migration emits these breaking schema identities without legacy aliases:

- scene-lab frame, cadence, manifest, measurement-pass, measurement, and
  assessment: v2;
- startup-intro qualification: v3;
- catalog-lifecycle installed benchmark: v2;
- installed screensaver benchmark: v5;
- particle benchmark, capacity, demo, step, and CPU-profile evidence: v2.

Scene, startup, particle, and screensaver qualification read `dropped_frames`
directly. Missing metrics, old schemas, and old metric names fail closed. Latch
drops, confirmation sequence failures, completion failures, long intervals,
and presentation failures remain independent results. For assessments, only
the unprofiled pass is authoritative; sampled cadence is attribution-only.

The staged-content terminology gate rejects deprecated metric identifiers in
production sources, reports, and current documentation. Explicitly marked
legacy-schema fixtures remain only to prove that old evidence is rejected.

## Device configuration

All runs used the installed Dev platform without modifying display state. Every
manifest recorded HDMI output and scan geometry at 1920×1200, with a 960×600
RGB565 framebuffer and render surface. The final typed status checks reported
`video_mode=1920,1200,60`, `LauncherActive`, a ready launcher, and
`arming=clear`.

The screenshot and card assessment passes used binary SHA-256
`58c0dc87b037571c72ff17e015ed10dfbaed3d8ee14bb23b75918fc364269e8f`.
The unprofiled navigation measurement used binary SHA-256
`7645015f6c22595e07c1a19e77be301173e3003e8e2beaf6d2078e35514cdef8`.
All receipts identify the same clean source commit above.

## Results

| Scene and pass | Duration | Unique FPS | Dropped frames | Sequence failures | Latch drops | Result |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Screenshot, unprofiled authority | 90 s | 59.952 | 0 | 0 | 0 | PASS |
| Screenshot, 99 Hz attribution | 90 s | 59.927 | 2 | 0 | 0 | attribution only |
| Card flip, unprofiled authority | 5 s | 59.951 | 0 | 0 | 0 | PASS |
| Card flip, 99 Hz attribution | 5 s | 59.949 | 0 | 0 | 0 | attribution only |
| Navigation `home-arcade`, unprofiled authority | 5 s | 52.706 | 36 | 0 | 0 | **FAIL — 36 dropped frames** |

The screenshot authoritative pass confirmed 5,397 frames over 5,396 expected
refresh intervals and recorded zero completion failures or long completion
intervals. The sampled pass recorded two 33.354 ms intervals; the assessment
correctly retained these as profiling attribution without allowing them to
control qualification.

The card-flip authoritative pass confirmed 301 frames over 300 expected refresh
intervals with zero completion failures or long intervals. Its dirty-region and
card-specific evidence remained populated.

Navigation confirmed 263 frames across 298 expected refresh intervals. Its 36
dropped frames matched 36 long confirmation intervals, while latch drops and
confirmation sequence failures remained zero. The typed command exited nonzero
and retained its bundle, proving that FPS tolerance and healthy latch counters
cannot compensate for dropped frames. This is the known navigation performance
defect; the terminology migration intentionally does not repair it.

## Diagnostic timing

| Scene and pass | Render mean / P99 | Process CPU | RSS mean / max |
| --- | ---: | ---: | ---: |
| Screenshot, authority | 6.875 / 8.607 ms | 81.27% | 28,324 / 36,716 KiB |
| Screenshot, sampled | 6.824 / 8.745 ms | 83.85% | 79,858 / 88,524 KiB |
| Card flip, authority | 3.743 / 6.183 ms | 39.68% | 4,020 / 4,052 KiB |
| Card flip, sampled | 3.925 / 6.484 ms | 45.06% | 55,552 / 55,584 KiB |
| Navigation, authority | 12.701 / 16.475 ms | 80.53% | 10,412 / 10,432 KiB |

The screenshot pack was read-only at
`/media/fat/mister-magik-dev/assets/arcade-screenshots-320x320.mmlz4b`, with
fingerprint `bytes=24326278
sha256=387728d3d0cf2aa2f2e5b8d56ecc72f63e8d4afc46ac97476dbd13d3ed360ee3`.
The deterministic seed was decimal `5575851515594434924`
(`0x4d6167694b54696c`).

## Commands and retained evidence

```text
scripts/agent device scene-lab --scene screenshot-screensaver --seconds 90 --assess --attended
scripts/agent device scene-lab --scene card-flip --seconds 5 --assess --attended
scripts/agent device scene-lab --scene navigation-transition --fixture home-arcade --seconds 5 --attended
scripts/agent device status
scripts/agent device mode status
```

- `build/scene-lab/screenshot-screensaver/1785941185/`
- `build/scene-lab/card-flip/1785941457/`
- `build/scene-lab/navigation-transition/1785941561/`

Each workflow restored the launcher. Explicit status and mode checks after the
short runs confirmed that the launcher was healthy and the 1920×1200 mode was
unchanged. The retained bundles contain v2 manifests, raw frame evidence,
machine summaries, and reports; assessment bundles additionally contain valid
profile metadata, folded stacks, and flamegraphs.
