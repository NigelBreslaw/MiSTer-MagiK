# Experimental raw-scaler boundary diagnostic

## Purpose

This is a disposable attribution RBF, not a production feature. It answers the
single question left by the coherent `0x0da3` scheduler capture: when the
physical sink is black and the authoritative RGB565 framebuffer is correct,
does the raw scaler boundary stop advancing, advance without active video,
emit active all-zero pixels, or emit substantial nonzero pixels?

The queued completion repair remains unchanged. The previous scheduler
observer is removed rather than retained or widened.

## Passive boundary

Command `0x67` now exposes `raw-scaler-boundary-v1`, schema `2`. Commands
`0x60` through `0x66` remain unsupported. Latch protocol `5` and capabilities
`0x03ff` remain unchanged.

The observer is clocked by `clk_hdmi` and reads only the existing `ascal`
outputs in `sys_top`: scaler clock enable, raw RGB, DE, HS, and VS. It has no
output other than the read-only UIO responder. No observer signal feeds the
scaler, latch, route, reset, PLL, mux, OSD, framebuffer, or pixel output logic.

The 16-bit state word is:

| Bits | Meaning |
| --- | --- |
| 0 | at least one completed-frame sample is valid |
| 1 | scaler clock enable was observed |
| 2 | horizontal sync was observed during the completed frame |
| 3 | reserved, must be zero |
| 7:4 | active-pixel samples, saturating at 15 |
| 11:8 | nonzero active-pixel samples, saturating at 15 |
| 15:12 | completed-frame sequence modulo 16 |

Saturating counts distinguish a single flashing pixel from a substantial
nonzero image without exporting RGB, fingerprints, geometry, or wide counters.

The HDMI-domain word and distinct preserved generation toggle are registered
before the generation changes. The
clk_sys responder synchronizes only that toggle through two explicit stages,
waits one additional clock, and samples the stable word. The command snapshot
remains immutable until the transaction ends and uses the existing CRC-16
evidence framing.

## Host classification

The device agent takes three records at 25 ms intervals. Validity and bounded
modulo-16 sequence deltas establish freshness; identical state is no longer
treated as proof of coherence.

| Classification | Required observation | Meaning |
| --- | --- | --- |
| `raw_scaler_timing_stalled` | both frame deltas are zero | HDMI-domain frame publication stopped; payload may be the last completed frame |
| `raw_scaler_no_active_video` | heartbeat advances, all active counts are zero | scaler timing advances without DE |
| `raw_scaler_black` | heartbeat advances, active counts are nonzero, nonzero counts are zero | raw scaler emits active all-zero RGB |
| `raw_scaler_sparse_or_corrupt` | heartbeat advances but activity is neither consistently black nor saturated nonzero | sparse pixel, transition, or corrupt evidence |
| `raw_scaler_active` | heartbeat advances and both counters saturate in all samples | raw scaler emits substantial nonzero RGB; investigate downstream |
| `raw_scaler_evidence_inconclusive` | invalid record or implausible/mixed freshness | repair only sampling or transport |

The record always reports `sink_visibility: "unobserved"`. Physical USB or
lossless analyzer evidence remains the authority for a black screen.

## Diagnostic quality gates

This experiment aims close to commercial implementation quality without
requiring a release campaign for hardware that will be removed:

- compile patched production `ascal.vhd`, not a mirrored substitute;
- keep exact-source queue BMC, all required covers, and induction passing;
- structurally prove the old scheduler observer is absent and raw observer
  outputs do not reach production assignments;
- simulate healthy, all-zero, missing-DE, sparse, stopped-clock-enable,
  sequence-wrap, reset, immutable command, CRC, malformed-read, and latch
  non-interference cases;
- require exact generation CDC endpoints, two synchronizer stages, bounded
  generation and bundled-data routes, and combined MTBF at least `10^12`
  device-hours;
- retain the production checker defaults of setup at least `0.428 ns`, hold at
  least `0.200 ns`, no more than `0.150 ns` matched-baseline degradation, and
  exact aggregate synchronizer-count growth for CI and release candidates;
- for this attended, rollback-capable local diagnostic only, permit setup at
  least `0.350 ns` and at most `0.300 ns` matched-baseline degradation while
  still requiring hold at least `0.200 ns`, zero TNS, every exact CDC
  assignment and endpoint, the exact calculable-chain delta, unchanged
  RAM/DSP/PLL identity, and no new warning class;
- retain the already accepted experimental 160 output-path count only; no
  timing waiver, seed sweep, fitter change, LogicLock, or production release
  claim is permitted;
- prefer the existing `+150` ALM and `+96` register budgets. A small documented
  diagnostic-only excess may be reviewed only if timing, CDC, and functional
  non-interference still pass; it is not a release waiver.

Only local Apple-container signoff selects the experimental profile. Direct CI
checker use remains production-strict. Only a committed candidate may enter
the cache. A passing local result permits the attended rollback-capable Dev
install and phase-2 reproduction; it does not qualify the RBF for production
or CI.

## Current status

The first phase-2 harness attempt was invalid. Arcade automation timed out
without launching a core or performing a return. A clock-only USB still was
incorrectly classified as the target black-screen failure; the physical
operator rejected that classification, and later capture showed the normal UI.

No root-cause inference is permitted from that attempt. Design 5 remains the
active experimental diagnostic candidate. Correct only the launcher/catalog
harness, then execute valid bounded launch/return transitions and wait for an
operator-confirmed failure. Exact hashes and the correction are in
[`history/2026-08-23-raw-scaler-boundary-black-result.md`](../history/2026-08-23-raw-scaler-boundary-black-result.md).
