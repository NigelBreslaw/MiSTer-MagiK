# Passive FPGA HDMI evidence

The current development candidate deliberately contains only the lossless
scaler completion-credit repair. It exposes no FPGA diagnostic command.

It removes the broad pixel-path classifiers, Avalon activity recorder, live
scaler-state export, and diagnostic extensions to the framebuffer latch. Those
features made the dense legacy scaler placement sensitive even when their own
timing paths were safe.

## Milestone A hardware contract

Commands `0x60` through `0x67` are explicitly unsupported. Canonical decoders
remain in the device agent solely for older qualified RBFs and rollback
compatibility; they do not describe this hardware.

The framebuffer latch and bridge are source-identical to the known qualified
split-responder implementation. They contain no diagnostic opcode decode,
snapshot bank, evidence CRC, or diagnostic port. The compatibility source paths
remain in the build input list but define no design unit, preserving invariant
cache identities without retaining hardware.

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
- both the pointer and its two forced destination synchronizers remain inside
  `ascal`, without a `sys_top` round trip;
- the destination recognizes a legal delta of zero, one, or two;
- a delta of two is consumed as two ordinary one-credit events on consecutive
  scaler-clock edges; and
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

The completion pointer is wholly internal functional state, not a diagnostic
payload exported through `sys_top`. Milestone A has no PLL-lock, pixel, Avalon,
or live scaler-state diagnostic taps. It uses no mailbox, block RAM, DSP, added
PLL output, placement directive, or functional timing exception.

## Collection and compatibility

Older qualified diagnostic RBFs can still be queried before rebooting or
reconfiguring the FPGA:

```text
scripts/agent device diagnostics --out PATH
```

Collection requires the authenticated agent transport, `LauncherActive`,
active MagiK latch ownership, and stable owner and launcher epochs across the
capture. Raw UIO is never read by the SSH fallback. Before every command the
agent completes an acknowledged strobe while IO enable is low, proving that
the FPGA command parser observed the transaction boundary.

Compatibility is explicit:

| Device agent | FPGA RBF | Result |
| --- | --- | --- |
| new | repair-only Milestone A | safe unavailable/unclassified result |
| new | older path-evidence recorder | existing detailed v2 JSON |
| new | older schema-4 observer | unchanged v1 fallback after explicit unsupported `0x60` |
| old | repair-only Milestone A | safe unavailable/unclassified result |
| old | older schema-4 observer | legacy v1 JSON |

## Qualification sequence

Milestone A is qualified independently. Its hard architecture ceilings are
150 ALMs and 96 fitted registers above the matched pre-observer baseline. It
must also satisfy:

- setup slack at least 0.428 ns;
- hold slack at least 0.200 ns;
- zero total negative slack;
- exactly 158 unconstrained output paths, equal to the baseline;
- exactly the two internal Gray synchronizer chains;
- exactly two applied Gray net-delay rows with nonnegative slack; and
- no RAM, DSP, PLL, warning-identity, passivity, or functional-cone regression.

The stock and pinned baseline variants are independently cached. A candidate
run must report both as cache hits and synthesize only the patched variant.
Failure of any Milestone A gate stops the staged redesign; Milestone B is not
built.

The known lock-only implementation at synthesis commit `840605cf` passed with
0.474 ns setup, 0.247 ns hold, zero TNS, 158 unconstrained outputs, and a delta
of 64 ALMs and 25 registers. Subsequent fit evidence showed that even this
independent responder materially amplified global fitted-register duplication
when combined with the repair, so it is deliberately absent here. Only the new
matched signoff can qualify the scaler repair.

A canonical local signoff may be installed only to the Dev layout through the
attended rollback-capable experimental FPGA transaction. It is not release
qualified. Production publication still requires the matched GitHub platform
workflow and normal release qualification.

The failed wide-observer and decode-timing experiments remain preserved in
[`history/2026-08-12-fpga-seed-2-video-diagnostics-retirement.md`](../history/2026-08-12-fpga-seed-2-video-diagnostics-retirement.md)
and
[`history/2026-08-14-fpga-diagnostic-decode-timing-experiment.md`](../history/2026-08-14-fpga-diagnostic-decode-timing-experiment.md).
