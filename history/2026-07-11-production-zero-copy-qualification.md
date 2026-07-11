# Production zero-copy qualification result

## Outcome

True zero-copy is functionally correct with the coherent mailbox, but it does
not beat the production Home performance gate when all required ownership and
cache transfers are enabled. It must not become the production default on the
current implementation.

## Confirmed cause

The old path copies broad Home damage from cached RAM into the write-combined
hidden buffer. The atomic path eliminates that copy but replaces it with DMA
cache cleaning for the same broad region and hard PTE revocation. On this stock
Cyclone V kernel/device, those required operations cost more than the copy they
remove.

## Before

Production Home `home-repeat-hold` median gate:

- work p99: 6888 us
- hidden copy p99: 1479–1512 us across the three baseline samples

## Correct production candidate

`PROD-ZC-HOME-STEADY-1-20260711`, 10 seconds:

- work p99: 8052 us
- delta from baseline: +1164 us (+16.9%)
- latch frames: 565/565
- fallback, visual misses, alternation failures, FPGA errors: 0
- atomic post p99: 2140 us
- Slint render p99: 5775 us

## Falsification experiments

Removing hard PTE revocation improved work p99 to 7455 us, still 567 us slower
than baseline and not production-safe. The change was reverted.

Preserving Slint's individual `PhysicalRegion` rectangles while retaining hard
ownership produced work p99 8081 us. Home damage is already broad, so this did
not reduce cache-clean cost. The change was reverted.

## Decision

- Keep the coherent mailbox, ownership API, diagnostics, and opt-in path.
- Do not flip production default.
- Do not weaken cache cleaning or hard ownership to manufacture a benchmark
  win.
- Reconsider only with a materially different mapping/revocation design or a
  workload whose real dirty ranges are sufficiently smaller.

Evidence:

- `history/2026-07-11-production-zero-copy-baseline.md`
- `build/launcher-home-scroll-profiles/PROD-ZC-HOME-STEADY-1-20260711-*`
- `build/launcher-home-scroll-profiles/PROD-ZC-NO-REVOKE-AB-20260711-*`
- `build/launcher-home-scroll-profiles/PROD-ZC-EXACT-DAMAGE-20260711-*`
