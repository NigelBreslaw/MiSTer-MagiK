# Catalog namespace overlap attribution

## Scope

This qualifies the fixed fd-relative namespace arm added at
`cd90a76bb37ee7c9d5c82e78f9fe0d4145bb538f` against the immediately preceding
exact-device control `44f64688fb0f4ba24c4272e8291fe063f3a13c59` on the same
dual-core Cortex-A9 MiSTer and exFAT card. The arm is diagnostic-only;
production namespace selection remains unchanged.

## Checklist

- [x] Captured a fresh current-HEAD baseline before instrumentation.
- [x] Pinned the diagnostic arm to fd-relative namespace traversal.
- [x] Recorded first entry, final entry, producer completion, consumer first
  work, and consumer completion for all 161 targets.
- [x] Recorded producer, channel wait, consumer wait, and consumer active time.
- [x] Recorded peak captured entries/bytes and buffer allocation count.
- [x] Recorded per-target and aggregate fallback/restart counts.
- [x] Verified 69 systems and 40,013 canonical games.
- [x] Verified exact identity, ordering, launch, search, and artifact-set hashes.
- [x] Preserved the production registry through isolated cleanup.

## Results

| Metric | Control | Instrumented arm | Absolute / percentage delta |
| --- | ---: | ---: | ---: |
| Observed workload | 165.730s | 141.550s | -24.180s (-14.6%) |
| Catalog scan | 92.116s | 69.486s | -22.631s (-24.6%) |
| Producer | 85.567s | 56.628s | -28.939s (-33.8%) |
| Channel wait | 0.566s | 4.673s | +4.107s (+726.0%) |
| Consumer wait | 32.790s | 9.155s | -23.635s (-72.1%) |
| Consumer active | 58.662s | 57.415s | -1.246s (-2.1%) |
| Process HWM | 115,236KiB | 115,356KiB | +120KiB (+0.1%) |

The time deltas are run-to-run attribution context, not a performance claim:
the committed change adds observers and an explicit diagnostic selector but no
new production traversal path. The candidate arm exposes a peak fd-relative
capture of 5,963 entries / 978,524 bytes and 11,649 namespace buffer
allocations. It observed zero fallbacks and zero restarts. The existing runtime
consumer batch still peaks at 18,915 files, independently of the namespace
capture peak.

All catalog behavior hashes are byte-identical across the two runs:

- identity: `0cdea8b6a2ea598c4175f92b15bbada2a7b788ad108d6e008fdcb1ebc9611b56`
- ordering: `bf5bda0a0218a75b0452d223764ac3c9a1bc373dbab81f49ba7ce0bfc34bd638`
- launch: `3f7d22b5c0b50f11fe2c00a0b7e5dff5cfda504abbf5e6aded5382b4320a5a32`
- search: `7c4f5e4a20a0f730623cb0691401c00e0aefb73e452eb97bec6802fedadd9ac7`
- artifact set: `844195ea432558bce9ec2e6cbf675d97ef529474aa58da1e46064049c8246ad0`

The measurements select restartable streaming as the next bounded hypothesis:
consumer wait remains 9.155s, the producer spends 56.628s before completing
the scan, and whole-target capture still performs 11,649 buffer allocations.
The consumer must first become restart-safe before any streamed entry can be
made authoritative.

Artifacts:

- Control: `build/agent-benchmarks/storage-attribution/1787365048/summary.json`
- Instrumented arm:
  `build/agent-benchmarks/storage-attribution/1787365663/summary.json`
