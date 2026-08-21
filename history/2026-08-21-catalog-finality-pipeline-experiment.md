# Catalog finality pipeline experiment — 2026-08-21

Revision tested: `3e7a2e53673b528bcdd95b6352e3c81a7a1adbee`

This experiment tested whether conservative contributor finality exposes enough
whole-card scan time to justify building system shards before the scan ends. It
temporarily drained wildcard targets first, then retained exact-system target
order. It did not start early shard writes.

## Result

Rejected before implementation of early writes. The proven overlap ceiling was
below the required 10 seconds.

- Wildcards drained and C64/PC-88 closed at 29.368 s.
- The scan execution pipeline ended at 34.582 s.
- Maximum possible expensive-shard overlap was therefore 5.214 s.
- The early Arcade shard took only 0.778 s to build and publish.
- GBA, GBC, MegaDrive, and NES closed with less than 1.5 s of scan remaining.

Building the early pipeline could not meet the retained experiment gate even
with perfect scheduling. Revision `05321adb1` therefore restores the qualified
production target order while retaining the exact-identity and contributor-
closure instrumentation.

## Whole-card evidence

`scripts/agent benchmark catalog-full-build-rebuild` passed:

| Leg | Complete | First visible | Peak HWM |
|---|---:|---:|---:|
| First observed clean | 186.136 s | 9.896 s | 139,032 KiB |
| Warm clean | 196.427 s | 15.890 s | 143,632 KiB |
| Forced rebuild | 51.778 s | 4.134 s | 62,544 KiB |

All legs retained 40,059 games, exact canonical/order/launch/search identities,
valid artifact SHA sets, and complete phase evidence. Peak HWM remained below
the 144,328 KiB ceiling.

The fresh result is 11.419 s faster than the immediately preceding qualified
closure run, but the warm result is 51.886 s slower and the earlier recorded
fresh baseline is faster still. The single fixed-order runs do not establish a
causal whole-wall improvement, so no performance claim is retained.

Evidence is in
`build/agent-benchmarks/catalog-full-build-rebuild/1787306191/summary.json` and
the adjacent per-leg launcher logs.
