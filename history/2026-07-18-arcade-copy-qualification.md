# Arcade scanout copy qualification

## Scope

This qualification covers the production RGB565 Arcade list presentation path on
the dual-core Cortex-A9 MiSTer. It does not cover experimental transition effects,
FPGA changes, Main changes, row-jump scenarios, or preview-cache rebuilding.

The test workload was a 30-second `human-turbo-hold` velocity scroll through the
real 903-game Arcade catalog, using the `release-device` launcher-scope binary and
the `fpga-vblank-latch-hidden` backend.

## Result retained on `main`

Two changes were accepted:

1. Each hidden scanout slot now tracks the Arcade content generation and cumulative
   vertical content offset. An alternating slot can therefore describe how far it
   is behind instead of treating every motion frame as unrelated content.
2. Cached Slint damage underneath the opaque Arcade layer no longer promotes a
   valid scroll update to a full update. The direct layer covers that damage, and
   latch planning subtracts the covered base restore region.

The actual viewport copy remains a RAM-to-write-combined-slot copy. The retained
change avoids redrawing the unchanged selection frame during scroll catch-up; it
does not read or shift the write-combined slot.

## Hardware evidence

| Run | Full updates | Common present bytes | Arcade compose p95 | Arcade compose p99 | Arcade compose max |
| --- | ---: | ---: | ---: | ---: | ---: |
| `ARCADE-COPY-C0-C3CAC588` | 1,782 | 694,472 | 1.441 ms | 1.727 ms | 3.056 ms |
| `ARCADE-COPY-C1-STATE` | 1,781 | 694,472 | 1.451 ms | 1.740 ms | 3.148 ms |
| `ARCADE-COPY-C2-PRESERVE` | 8 | 687,776 | 1.423 ms | 1.608 ms | 2.356 ms |

Relative to the original baseline, the accepted result reduced Arcade compose p95
by 1.2%, p99 by 6.9%, and the observed maximum by 22.9%. The common presentation
was 6,696 bytes smaller. Entry, preview exactness, composition recovery, frame
pacing, and FPGA latch-drop gates passed; there were no fallback, timeout, error,
row-drop, or latch-drop frames.

The warm-cache baseline did not emit a fresh search-index lifecycle, so its search
overlap gate was inapplicable. A forced-refresh control run was rejected because
the benchmark reported missing runtime composition status. Matched rendering runs
therefore used the benchmark's explicit `--skip-search-overlap-gate`; no catalog or
search code changed in this series.

Raw artifacts are under `build/arcade-scroll-profiles/` and remain ignored.

## Rejected candidates

The non-inverted viewport batching fast path was not retained. Run
`ARCADE-COPY-C3-BATCH` regressed p95 from 1.423 to 1.457 ms, p99 from 1.608 to
1.696 ms, raised the maximum to 5.528 ms, and grew the binary by 4 KiB.

Write-combined shift-and-repair was rejected before production retention because
it cannot meet the bytes-written gate: shifting a 510 by 480 RGB565 viewport must
rewrite the retained pixels in the slot and then repair exposed and selection
bands. It also introduces write-combined reads, which prior device evidence found
slower than RAM-to-slot rewriting. A future reduced-band implementation needs a
hardware scanout-offset primitive or a cacheable scanout slot; it should not be
hidden behind a runtime fallback in the current production path.

## Production rule

Keep the current direct RAM-to-hidden-slot copy as the single production route.
Only revisit narrower Arcade damage writes when the platform can preserve shifted
pixels without CPU reads and rewrites of the write-combined scanout slot. Qualify
any replacement with the same real velocity scenario and require exact pixels,
no latch or presentation failures, lower p95 and p99, a bounded maximum, and no
increase in write-combined bytes.
