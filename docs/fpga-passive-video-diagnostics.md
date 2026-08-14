# Passive FPGA HDMI evidence

The current development candidate deliberately returns to a small, staged
architecture. Milestone A contains only:

- the previously qualified, independent physical HDMI FPLL-lock recorder; and
- the lossless scaler completion-credit repair.

It removes the broad pixel-path classifiers, Avalon activity recorder, live
scaler-state export, and diagnostic extensions to the framebuffer latch. Those
features made the dense legacy scaler placement sensitive even when their own
timing paths were safe. Milestone B may add compact repair evidence only after
Milestone A independently passes fixed-seed qualification.

## Milestone A hardware contract

UIO command `0x60` returns the unchanged `hdmi-lock-evidence-v1` four-word
record:

1. schema;
2. lock flags;
3. saturated lock-loss count;
4. CRC-16/CCITT-FALSE.

The flags report whether lock was seen high, whether two consecutive
synchronized high samples armed the recorder, the current synchronized lock
state, whether lock was ever lost after arming, and counter overflow. Reads
snapshot the record atomically and never arm, clear, reset, or otherwise mutate
the recorder.

Commands `0x61` through `0x67` are explicitly unsupported by Milestone A.
The device agent treats unsupported richer evidence as lock-only capability; a
malformed, wrong-schema, or CRC-invalid supported record remains an error.
The canonical decoders for older qualified path-evidence and schema-4 RBFs are
retained solely for rollback compatibility and do not describe Milestone A
hardware.

The framebuffer latch and bridge are source-identical to the known qualified
split-responder implementation. They contain no diagnostic opcode decode,
snapshot bank, evidence CRC, or diagnostic port. The `0x60` recorder is an
independent top-level read-only responder with a statically disjoint opcode.

## Lossless completion repair

The legacy scaler allowed two framebuffer-read blocks to be outstanding, but
reported completed blocks across the `clk_100m` to `clk_hdmi` boundary with one
toggle. If `clk_hdmi` stopped while both blocks completed, two source toggles
could cancel before the destination sampled either. The scaler could then
retain two outstanding reads permanently, produce black active video, and stop
issuing new requests.

Milestone A replaces only that completion crossing:

- the source registers a two-bit modulo-4 Gray completion pointer at the
  existing completed-block point;
- each Gray bit crosses through its own forced two-stage synchronizer;
- the destination computes a legal delta of zero, one, or two;
- a delta of two is serialized into two ordinary one-credit pulses; and
- the scaler retains its original copy-level update truth table and metadata
  ordering.

The maximum unseen delta is two because the unchanged scaler cannot issue more
than two outstanding blocks. Delta three is invalid and produces no fabricated
credit. The existing one-outstanding request-accept toggle remains safe because
a second request cannot be issued until the first acceptance has crossed.

The repair adds no reset, PLL, SDRAM, latch, route, or pixel-control behavior.
It does not reset the scaler on lock loss. The exact two registered Gray-source
paths to the first destination stages have a checker-required 10 ns net-delay
bound. No new false path or multicycle exception is introduced.

## Clock-domain and passivity rules

The lock source is the existing physical `reconfig_from_pll[16]` status bit.
Its only raw diagnostic consumer is the first stage of a two-register
`clk_sys` synchronizer. The sole diagnostic false path ends at that first-stage
data pin; the second stage remains timed and must appear in metastability
analysis.

Apart from the two-bit Gray completion pointer, Milestone A has no diagnostic
payload crossing and no pixel, Avalon, or live scaler-state taps. It uses no
mailbox, block RAM, DSP, added PLL output, placement directive, or functional
timing exception. Observer outputs can reach only the read-only UIO response
mux.

## Collection and compatibility

Collect evidence before rebooting or reconfiguring the FPGA:

```text
scripts/agent device diagnostics --out PATH
```

Collection requires the authenticated agent transport, `LauncherActive`,
active MagiK latch ownership, and stable owner and launcher epochs across the
capture. Raw UIO is never read by the SSH fallback. Before every command the
agent completes an acknowledged strobe while IO enable is low, proving that
the FPGA command parser observed the transaction boundary.

The lock classifications remain:

- `hdmi_pll_lock_lost`;
- `hdmi_pll_locked`;
- `hdmi_pll_not_stably_armed`; and
- `hdmi_pll_not_seen`.

Compatibility is explicit:

| Device agent | FPGA RBF | Result |
| --- | --- | --- |
| new | Milestone A | v2 lock-only JSON from `0x60` |
| new | older path-evidence recorder | existing detailed v2 JSON |
| new | older schema-4 observer | unchanged v1 fallback after explicit unsupported `0x60` |
| old | Milestone A | safe unavailable/unclassified result |
| old | older schema-4 observer | legacy v1 JSON |

## Qualification sequence

Milestone A is qualified independently. Its hard architecture ceilings are
150 ALMs and 96 fitted registers above the matched pre-observer baseline. It
must also satisfy:

- setup slack at least 0.428 ns;
- hold slack at least 0.200 ns;
- zero total negative slack;
- exactly 158 unconstrained output paths, equal to the baseline;
- the exact lock, reset-release, and two Gray synchronizer chains;
- exactly two applied Gray net-delay rows with nonnegative slack; and
- no RAM, DSP, PLL, warning-identity, passivity, or functional-cone regression.

The stock and pinned baseline variants are independently cached. A candidate
run must report both as cache hits and synthesize only the patched variant.
Failure of any Milestone A gate stops the staged redesign; Milestone B is not
built.

If Milestone A passes, Milestone B may add a second independent sidecar for
compact completion-repair evidence. It must not modify the qualified `0x60`
recorder or latch/bridge, restore pixel/Avalon classifiers, or export a live
scaler-state bus. Milestone B receives its own simulation, structural review,
commit, and single fixed-seed signoff. A Milestone B failure leaves the
qualified Milestone A repair as the fallback.

The known lock-only implementation at synthesis commit `840605cf` passed with
0.474 ns setup, 0.247 ns hold, zero TNS, 158 unconstrained outputs, and a delta
of 64 ALMs and 25 registers. Those numbers justify the split boundary but do
not qualify the new scaler repair; only the new matched signoff can do that.

A canonical local signoff may be installed only to the Dev layout through the
attended rollback-capable experimental FPGA transaction. It is not release
qualified. Production publication still requires the matched GitHub platform
workflow and normal release qualification.

The failed wide-observer and decode-timing experiments remain preserved in
[`history/2026-08-12-fpga-seed-2-video-diagnostics-retirement.md`](../history/2026-08-12-fpga-seed-2-video-diagnostics-retirement.md)
and
[`history/2026-08-14-fpga-diagnostic-decode-timing-experiment.md`](../history/2026-08-14-fpga-diagnostic-decode-timing-experiment.md).
