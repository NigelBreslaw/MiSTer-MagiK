# Cache-preserving full raster production qualification — 2026-08-22

## Authority

- Installed experiment revision: `6972d97cdd77e4311cbcaefa64fd9bd3a34a4b2a`
- Main revision: `639d3694e1b93660020e9587cd0fe27f0170ce4c`
- Display: `hdmi-1280x720p60`
- Performance authority: unprofiled installed Dev runtime
- Control policy: `NewBuffer`
- Candidate policy: full-region dirty marking with `ReusedBuffer`

The corrected benchmark retains the immediate physical frame, then continues
through the first real ordinary Slint render. It reports every forced-full
raster through that first render, resetting the window if another forced raster
supersedes it.

## Paired exact-device results

| Pair | Order | NewBuffer combined | Reused combined | Delta |
| --- | --- | ---: | ---: | ---: |
| 1 | control, candidate | 32,767us | 17,819us | -14,948us (-45.6%) |
| 2 | candidate, control | 33,557us | 17,364us | -16,193us (-48.3%) |
| 3 | control, candidate | 34,347us | 17,450us | -16,897us (-49.2%) |
| Median | alternating | 33,557us | 17,450us | -16,107us (-48.0%) |

Median forced-raster time moved from 16,201us to 16,504us, a 303us (1.9%)
cost for retaining and refreshing the partial-render cache. Median subsequent
recovery raster time fell from 16,751us to 946us, a 15,805us (94.4%)
improvement. Duplicate full recovery rasters fell from two to zero in every
candidate run.

The Settings destination-through-recovery median improved from 20,129us to
12,931us, a 7,198us (35.8%) reduction.

## Correctness and disposition

All six arms produced the identical authoritative terminal Settings PNG hash:
`42e7ef05f7510300246df562e67c54c9de481cf6abc70651db0917e40eabd58c`.
Every arm passed with zero physical repeated vblanks, latch drops, ownership
losses, sequence gaps, or phase outliers.

The earlier 4.8% rejection compared the forced Settings destination with an
immediate physical frame that did not invoke Slint. It therefore did not test
cache recovery. The corrected metric proves a 48.0% median improvement and
passes the 20% gate in all three paired runs. Promote cache-preserving full
raster for reusable backing buffers. Retain `NewBuffer` only as the explicit
fallback for newly allocated or otherwise discontinuous backing storage.

## Artifacts

- Pair 1 control: `build/agent-benchmarks/settled-composition/1787398700/summary.json`
- Pair 1 candidate: `build/agent-benchmarks/settled-composition-reused-cache/1787398730/summary.json`
- Pair 2 candidate: `build/agent-benchmarks/settled-composition-reused-cache/1787398763/summary.json`
- Pair 2 control: `build/agent-benchmarks/settled-composition/1787398786/summary.json`
- Pair 3 control: `build/agent-benchmarks/settled-composition/1787398817/summary.json`
- Pair 3 candidate: `build/agent-benchmarks/settled-composition-reused-cache/1787398839/summary.json`

## Production-default controls

Revision `a11689dcc7a5835e4c28ef6d6eddd777428bf8eb` was delivered after selector
removal. Its ordinary `settled-composition` run reported policy
`reused-buffer`, 17,979us combined recovery work, 927us subsequent recovery,
zero duplicate full recovery rasters, the same terminal PNG hash, and no
presentation fault:

- `build/agent-benchmarks/settled-composition/1787399178/summary.json`

Four unprofiled Settings navigation controls exercised six landscape and six
portrait-left transitions each. The first run recorded two physical repeats on
portrait-left Home return despite only 4,026us maximum work in that leg. The
next three complete controls passed all 36 legs with zero physical drops. This
matches the benchmark's retained intermittent portrait-repeat history and is
not attributable to a frame-work overrun:

- isolated failure: `build/agent-benchmarks/settings-navigation/1787399205/`
- confirmations: `build/agent-benchmarks/settings-navigation/1787399288/`,
  `1787399344/`, and `1787399414/`

The dedicated six-leg brightness-fade orientation qualification passed normal,
clockwise, and counterclockwise transitions with zero repeated vblanks, latch
drops, ownership losses, or sequence gaps; maximum whole-frame work was
9,341us:

- `build/agent-benchmarks/orientation-transition-fade/1787399597/`

The modal-input control also passed exact modal ownership, held-input release,
and direct-layer retirement/restoration behavior:

- `build/agent-benchmarks/modal-input/1787399679/`

The general `navigation-transitions` runner did not reach its route in two
attempts because its shared screensaver profiling prerequisite timed out. These
runs produced no cache, pixel, or cadence result and are recorded as an
unrelated benchmark-runner issue:

- `build/agent-benchmarks/navigation-transitions/1787399468/`
- `build/agent-benchmarks/navigation-transitions/1787399534/`

A focused diagnostic keepalive recovery was delivered and tested at revision
`42f4b2309869a6a552c8e93745bb16689aafaefc`. It also timed out, proving the
profiler never reached the warming/active state where a keepalive could apply.
The recovery was therefore reverted. The dominant unresolved phase is earlier:
the scripted route does not arm the bounded navigation profiler.

After the explicit recovery revert, final Dev revision
`405caabef6aa981b883545963f6a5fbe5dce9580` was delivered and rerun through
ordinary selector-free `settled-composition`. It reported 17,553us combined
recovery work, 942us subsequent recovery, zero duplicate full recovery rasters,
the authoritative terminal PNG hash, and zero presentation faults:

- `build/agent-benchmarks/settled-composition/1787400104/summary.json`
