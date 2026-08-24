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

For every completed frame it derives a CRC-16 of the ordered four-bit
`{CE, DE, HS, VS}` sample stream. The complete waveform fingerprint retains
ordering, phase, sync, enable, and active-area changes without duplicating wide
per-frame counters.

A frame is nonempty only after CE, HS, VS, and DE were all observed. Three
consecutive identical, nonempty fingerprints establish the baseline. After
that, the first fingerprint difference or empty frame is latched. The
baseline and first-bad record remain immutable until the existing common reset
or RBF reload. There is no software clear, arm, freeze, write, or recovery
operation.

## Read-only ABI

Command `0x67` exposes `raw-scaler-frame-integrity-v1`, schema `3`. Commands
`0x60` through `0x66` remain unsupported. Latch protocol `5` and capabilities
`0x03ff` are unchanged.

After magic `0x4d57`, the fixed five-word response is:

| Word | Meaning |
| --- | --- |
| 0 | schema `3` |
| 1 | flags: sample valid, nonempty, baseline valid, mismatch latched |
| 2–3 | baseline and first-bad control CRC |
| 4 | existing framed CRC-16 |

The HDMI bundle is registered before its generation toggle changes. The
`clk_sys` receiver uses the explicit two-stage generation synchronizer, waits
one additional clock, and snapshots the stable bundle. A command transaction
then reads an immutable snapshot.

## Host classification

The device agent reads three records 25 ms apart and requires valid framing,
identical records, a valid baseline, stable ownership, and stable launcher
state. A retained mismatch remains classifiable even when the bad frame itself
was empty.

| Classification | Meaning |
| --- | --- |
| `raw_control_mismatch_latched` | the observer retained a first control-waveform mismatch |
| `raw_control_stable_since_baseline` | no raw control mismatch has been retained; a later probe should move downstream |
| `raw_frame_integrity_inconclusive` | evidence is unsupported, malformed, changing, invalid, empty, or lacks a baseline |

Every result retains `sink_visibility: "unobserved"`. Stable control evidence
does not prove pixels or the physical sink were correct.

## Local gates before synthesis

- Apply the production patch and compile patched production `ascal.vhd`.
- Keep the exact completion-queue GHDL simulation and formal proof passing.
- Simulate exact three-frame baseline acquisition, changing and empty frames,
  phase-only CRC mismatch and independent HS/DE/active waveform changes,
  sticky first-bad retention, reset, immutable command framing,
  CRC, malformed reads, and latch-v5 non-interference.
- Structurally reject the retired RGB/activity observer, production fanout, or
  any new framebuffer, latch, route, reset, PLL, mux, or pixel tap.
- Require exact generation synchronizer endpoints, bounded generation and
  bundled-data paths, and the existing MTBF policy.
- For this disposable diagnostic profile only, cap growth at 208 ALMs and 224
  registers while still requiring unchanged RAM, DSP, and PLL identity. Keep
  the 0.200 ns hold floor and zero-TNS requirement.
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

## Phase 2 result

The signed schema-3 RBF reproduced a persistent physical black screen on boot
epoch 3, attempt 5 after 44 clean direct-Arcade returns and two supervised
reboots. The USB-video still was uniform video-level black and byte-identical
to the earlier confirmed black incident. At the same checkpoint, the
authoritative RGB565 framebuffer was byte-identical to the known-correct
Arcade return.

The failure-time record and a second live three-sample record both remained
`raw_control_stable_since_baseline`: baseline CRC `45489`, first-bad CRC `0`,
three identical coherent samples, stable ownership, and zero latch drops or
rejects. No reboot, RBF reload, launcher restart, or additional transition was
performed after detection.

This result rejects a CE/DE/HS/VS waveform change at the observed raw-scaler
boundary as the cause of this black incident, within the CRC probe's coverage.
It does not cover RGB pixel data. The next diagnostic should retire this probe
and move to one equally passive, sticky fingerprint at the next downstream
boundary. See
[`history/2026-08-24-frame-integrity-black-result.md`](../history/2026-08-24-frame-integrity-black-result.md).
