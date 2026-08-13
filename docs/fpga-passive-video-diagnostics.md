# Passive FPGA HDMI evidence

The current diagnostic is a bounded, read-only recorder for the physical HDMI
FPLL lock and four video-path boundaries. After a rare black return to Menu it
distinguishes the final direct/scaled mux selection, raw scaler activity,
post-OSD activity, and bounded framebuffer-read transport liveness.

The video recorders remain passive. The current candidate also repairs one
specific scaler CDC defect: two completed Avalon return blocks can no longer
cancel while the dynamically generated HDMI clock is stopped. It does not
change the framebuffer latch, SDRAM interface, reset, PLL reconfiguration, or
pixel algorithms.

## Hardware contract

UIO command `0x60` returns `hdmi-lock-evidence-v1`, a fixed four-word record:

1. schema;
2. lock flags;
3. saturated lock-loss count;
4. CRC-16/CCITT-FALSE.

The flags report whether lock was seen high, whether two consecutive
synchronized high samples armed the recorder, the current synchronized lock
state, whether lock was ever lost after arming, and counter overflow. A
one-sample high pulse records `seen_high` but does not arm or create a false
loss. Reads snapshot the record atomically and never arm, clear, or otherwise
mutate it.

UIO command `0x61` returns `hdmi-output-activity-v1`, a fixed six-word record:

1. schema;
2. validity and collision flags;
3. completed-frame no-DE epoch;
4. completed-frame DE-all-zero epoch;
5. completed-frame DE-has-nonzero epoch;
6. CRC-16/CCITT-FALSE.

The three epochs are independent four-bit modulo counters. The HDMI-domain
classifier observes only the registered `hdmi_out_vs`, `hdmi_out_de`, and
`hdmi_out_d` values that drive the transmitter pins. The first rising VS arms
the classifier and discards the partial startup interval. Every later rising
VS classifies the preceding interval into exactly one category. A sticky
collision flag makes any impossible simultaneous destination events explicit.
Reads snapshot all counters atomically and never change the recorder.

Commands `0x62`–`0x66` add strict, CRC-protected records without changing
`0x60` or `0x61`:

- `0x62` counts final-output no-DE, nonzero, black-direct, black-scaled, and
  black-mixed completed frames;
- `0x63` counts no-DE, all-zero, and nonzero frames at raw scaler output;
- `0x64` counts the same classes after OSD/shadow-mask processing;
- `0x65` counts bounded Avalon buckets, plus buckets containing a read request,
  an accepted request, or returned data. The bucket epoch makes a valid empty
  interval distinguishable from a stopped source clock;
- `0x66` reports a frame-held scaler scheduler/copy state, completion-credit
  recovery epochs, and full-line/full-frame two-read starvation epochs.

Every record is snapshotted at command recognition and read-only. The agent
reads each detailed record twice about 50 ms apart and computes four-bit
modular deltas independently; it never assumes that events from different
clock domains describe the same numbered frame.

A complete producer reports diagnostic architecture `video-path-evidence-v1`.
If `0x62` is explicitly unsupported, a new agent retains the earlier `0x60`/
`0x61` interpretation; `0x66` is optional for older path-evidence RBFs, while a
malformed supported record is always an error.

Commands `0x5D`–`0x5F` belong to the retired schema-4 three-domain observer and
are unsupported by the current FPGA. New device software probes `0x60` first
and falls back to the old records only when `0x60` is explicitly unsupported,
so older qualified RBFs remain diagnosable. A malformed, unknown-schema, or
CRC-invalid `0x60` response is an error and never triggers legacy fallback.

Compatibility is explicit:

| Device agent | FPGA RBF | Result |
| --- | --- | --- |
| new | new path-evidence recorder | v2 JSON from `0x60`–`0x66` |
| new | older lock-only recorder | v2 lock-evidence JSON; final-output capability unavailable |
| new | older schema-4 observer | unchanged v1 JSON after explicit unsupported fallback |
| old | new lock recorder | safe unavailable/unclassified result because `0x5D`–`0x5F` are unsupported |
| old | older schema-4 observer | legacy v1 JSON |

The unavailable/architecture-unknown envelope intentionally retains schema v1
for compatibility. Only a successfully decoded `0x60` record emits the v2
lock-evidence shape.

## Clock-domain boundary

The physical status source is the existing `reconfig_from_pll[16]` signal. The
stock `pll_hdmi` wrapper keeps its redundant lock output terminated; generated
Intel IP is unchanged.

The PLL status has exactly one raw consumer: the first register in a two-stage
`clk_sys` synchronizer. The only diagnostic timing exception is a false path to
that first-stage data pin. The first-to-second-stage settling path remains
timed and must appear in Quartus metastability analysis.

The final classifier emits five mutually exclusive event toggles. Raw scaler
and post-OSD classifiers emit three each. The Avalon recorder emits three
liveness-category toggles plus one bucket heartbeat. Every toggle has its own
forced two-stage `clk_sys` synchronizer; only synchronized changes are counted.
The scaler repair replaces the lossy return-completion toggle with a registered
two-bit Gray completion sequence. Two forced destination synchronizer chains
recover a modulo delta of one or two and serialize it back into the scaler's
original one-credit pulse interface. A two-bit max-skew constraint bounds only
that Gray bus; there is no new false path or multicycle exception. The 0x66
state vector is held for a complete scaler frame and double-sampled before a
command snapshots it. The design uses no mailbox, block RAM, or DSP and never
observes Avalon addresses or returned pixel data.

## Collection and interpretation

Collect evidence before rebooting or reconfiguring the FPGA:

```text
scripts/agent device diagnostics --out PATH
```

The authenticated device agent uses the existing UIO advisory lock and writes
`fpga-video-diagnostics.json` into the support bundle. Raw UIO is never read by
the SSH fallback. Collection requires the agent transport, `LauncherActive`,
active MagiK latch ownership, and stable owner epoch, launcher state, and latch
ownership across the capture. Otherwise the bundle records the evidence as
unavailable or incoherent rather than issuing an unowned FPGA transaction.
Before each UIO command, the agent lowers IO enable and completes an
acknowledged strobe while it remains disabled. That acknowledgement proves the
disabled state crossed the HPS-to-`clk_sys` synchronizer and reset the FPGA
command parser before the next opcode is raised.

The lock classifications are:

- `hdmi_pll_lock_lost`: lock was lost at least once after stable arming;
- `hdmi_pll_locked`: lock is currently high and no loss was retained;
- `hdmi_pll_not_stably_armed`: a high sample was seen but stable arming was not;
- `hdmi_pll_not_seen`: lock has not been sampled high in this configuration.

The device agent reads `0x61` twice about 50 ms apart and computes modulo
deltas. Its final-output classifications are:

- `final_output_no_completed_frame`: no completed final-output frame crossed
  during the sample window;
- `final_output_no_de`: completed frames advanced without active DE;
- `final_output_de_all_zero`: DE was present but every active RGB sample was
  zero;
- `final_output_de_has_nonzero`: at least one active RGB sample was nonzero;
- `final_output_mixed`: more than one class occurred during the window;
- `final_output_activity_invalid`: the impossible-event collision flag was set.

`final_output_de_has_nonzero` proves only nonzero digital activity at the final
registered FPGA boundary. It does not prove pixel correctness or downstream
HDMI transmitter, PHY, cable, capture-device, or display visibility. Likewise,
an unchanged epoch proves no completed frame, not that the HDMI clock itself
stopped.

The detailed classification is deliberately conservative. It requires one
stable final class across at least two completed frames. Black-direct localizes
the fault to the direct mux selection. Black-scaled is then divided by raw and
post-OSD activity: raw nonzero followed by post-OSD black localizes the fault to
post-scaler processing; post-OSD nonzero followed by final black localizes it to
final staging. Raw scaler black is combined with the Avalon bucket deltas to
report no requests, blocked requests, accepted requests without observed
returns, or active returns while the scaler remains black. Mixed, colliding,
nonadvancing, or insufficient windows remain inconclusive rather than being
guessed.

The optional scaler-fetch record is interpreted separately. A stable
`sREAD`, read level two, copy level zero state together with complete-frame
starvation epochs is reported as `scaler_fetch_stalled_with_two_reads`.
Two-credit catch-up epochs show that the repaired crossing recovered both
completions after a destination-clock pause. Full-line starvation is reported
observationally and may help correlate intermittent vertical bands; it is not
by itself a claim that those bands share the black-screen cause.

## Qualification

The lock-only fixed-seed-2 local signoff at synthesis commit `840605cf` and
assurance-complete commit `23b5f5d2` passed the unchanged hard timing gates:

- setup slack: 0.474 ns;
- hold slack: 0.247 ns;
- total negative slack: zero;
- unconstrained output paths: 158, equal to the pinned pre-observer build;
- one added calculable synchronizer chain;
- resource delta: 64 ALMs, 25 registers, no block memory, no DSP.

The product ceiling remains 800 ALMs. The register design target is 300 and the
predeclared hard ceiling for the repair plus coherent 0x66 evidence is 360.
The lock-only local build used 64 ALMs;
the independently fitted GitHub build used 105 ALMs. Both are comfortably
inside the intended architecture budget. Any synthesized-source change still
requires fresh empirical signoff. Setup and hold must remain at least 0.20 ns,
TNS zero, slack degradation no more than 0.15 ns, warnings within the pinned
identity, and unconstrained output paths equal to the pre-observer build.

The detailed path extension is a new synthesized-source change and is not
qualified by those lock-only numbers. It must pass a fresh matched local and
GitHub signoff with the exact declared synchronizer inventory, no new
unconstrained output paths, and the 800-ALM/360-register hard ceiling before any
device installation. A result above 300 registers requires fitted hierarchy
attribution to this repair/evidence scope rather than unexplained duplication.

Quartus's aggregate auto-detected synchronizer count is recorded for comparison
but is not an exact delta gate: unrelated Menu chains can be combined or split
by fitter placement. Qualification instead requires all exact observer stage
assignments, all named chains in the retained metastability report, and exactly
the predeclared additional chains with calculable MTBF.

A canonical local signoff set may be installed only to the Dev layout through
the attended rollback-capable experimental FPGA transaction. It is not release
qualified. Production publication still requires the matched GitHub platform
workflow and normal release qualification.

The transaction activates the installed Dev latch RBF only through Main-owned
`load_core` with the exact Dev manifest path. Root `/media/fat/menu.rbf` is the
stock `update_all` artifact; neither that pathname, its compatibility redirect,
nor a `mister_magik_reload_main` process replacement is valid activation
evidence for an experimental RBF.

Device acceptance must exercise the new RBF's v2 lock and final-output records, an older qualified
schema-4 RBF's unchanged v1 fallback, and the SSH unavailable path. Each test
must verify that CRC/semantic failures never fall back and that SSH never reads
raw UIO.

The failed wide-observer attempts and their rejected RBF evidence remain in
[`history/2026-08-12-fpga-seed-2-video-diagnostics-retirement.md`](../history/2026-08-12-fpga-seed-2-video-diagnostics-retirement.md).
