# Experimental raw-scaler frame-integrity diagnostic

This disposable RBF observer preserves the first raw-scaler control-timing
anomaly after a stable baseline. It exists to classify the next physical black
or corrupt MagiK frame; it is not a production feature or recovery mechanism.

The queued completion repair is unchanged. The schema-2 RGB/activity observer
is removed rather than stacked with this design.

## Passive boundary and retained state

The HDMI-domain observer reads only the existing raw scaler `CE`, `DE`, `HS`,
and `VS` controls. It does not read RGB, framebuffer addresses, metadata, final
output, routing, latch, reset, or PLL signals. Its output reaches only the
read-only responder beside the latch bridge in `sys_top`.

For every completed frame it derives:

- CRC-16 of every ordered `{CE, DE, HS, VS}` control sample;
- rising HS edge count;
- rising DE/start count;
- active `CE && DE` sample count.

A frame is nonempty only after CE, HS, VS, and DE were all observed. Three
consecutive identical, nonempty, non-overflowing tuples establish the baseline.
After that, the first tuple difference, empty frame, or counter overflow is
latched with its modulo-65536 frame sequence and observation generation. The
baseline and first-bad record remain immutable until the existing common reset
or RBF reload. There is no software clear, arm, freeze, write, or recovery
operation.

## Read-only ABI

Command `0x67` exposes `raw-scaler-frame-integrity-v1`, schema `3`. Commands
`0x60` through `0x66` remain unsupported. Latch protocol `5` and capabilities
`0x03ff` are unchanged.

After magic `0x4d57`, the fixed 15-word response is:

| Word | Meaning |
| --- | --- |
| 0 | schema `3` |
| 1 | flags: sample valid, nonempty, overflow, baseline valid, mismatch latched |
| 2–3 | baseline and first-bad control CRC |
| 4–5 | baseline and first-bad HS edge count |
| 6–7 | baseline and first-bad DE-start count |
| 8–9 | baseline 24-bit active sample count, low then high |
| 10–11 | first-bad 24-bit active sample count, low then high |
| 12 | first-bad frame sequence |
| 13 | first-bad observation generation |
| 14 | existing framed CRC-16 |

The HDMI bundle is registered before its generation toggle changes. The
`clk_sys` receiver uses the explicit two-stage generation synchronizer, waits
one additional clock, and snapshots the stable bundle. A command transaction
then reads an immutable snapshot.

## Host classification

The device agent reads three records 25 ms apart and requires valid framing,
identical records, a valid baseline, stable ownership, and stable launcher
state. A retained mismatch remains classifiable even when the bad frame itself
was empty or overflowed a diagnostic counter.

| Classification | Meaning |
| --- | --- |
| `raw_control_mismatch_latched` | the observer retained a first control-waveform or count mismatch |
| `raw_control_stable_since_baseline` | no raw control mismatch has been retained; a later probe should move downstream |
| `raw_frame_integrity_inconclusive` | evidence is unsupported, malformed, changing, invalid, empty, overflowing, or lacks a baseline |

Every result retains `sink_visibility: "unobserved"`. Stable control evidence
does not prove pixels or the physical sink were correct.

## Local gates before synthesis

- Apply the production patch and compile patched production `ascal.vhd`.
- Keep the exact completion-queue GHDL simulation and formal proof passing.
- Simulate exact three-frame baseline acquisition, changing and empty frames,
  phase-only CRC mismatch with equal counts, each independent count mismatch,
  sticky first-bad retention, sequence wrap, reset, immutable command framing,
  CRC, malformed reads, and latch-v5 non-interference.
- Structurally reject the retired RGB/activity observer, production fanout, or
  any new framebuffer, latch, route, reset, PLL, mux, or pixel tap.
- Require exact generation synchronizer endpoints, bounded generation and
  bundled-data paths, and the existing MTBF policy.
- Do not run CI or install this RBF until the committed candidate passes the
  cached Apple-container signoff. No seed sweep, waiver, fitter change,
  LogicLock, or direct Quartus command is permitted.

## Why this observer replaces schema 2

On 2026-08-24 one corrupt native frame appeared between byte-identical healthy
MagiK frames while the authoritative framebuffer stayed correct. The old
activity snapshots remained saturated and active, and the subsequent
755-frame native movie was entirely healthy. The anomaly therefore disappeared
before a delayed movie or non-sticky activity record could classify it.

This design retains the first raw control-timing anomaly across self-recovery.
If no mismatch is retained during a confirmed physical failure, the raw
control boundary is excluded only within this probe's limits and the next
minimal observer moves downstream. The preserved incident is documented in
[`history/2026-08-24-phase2-transient-corruption-result.md`](../history/2026-08-24-phase2-transient-corruption-result.md).
