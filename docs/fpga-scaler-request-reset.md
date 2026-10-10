# Scaler request reset repair

A reset at a completed high request phase could create an uncharged read. The
output reset cleared `o_read_pre` but retained the transmitted `o_read`. The
Avalon receiver also retained its synchronizer stages and registered pulse.
After release, the transition to zero became another request. Its copy retired
with no output read credit, wrapping the two-bit `o_readlev` from zero to three.

The repair adds seven assignments to the existing reset branches:

- Output: `o_read`, `o_readack_sync`, `o_readack_sync2`, `o_readack`.
- Avalon: `avl_read_sync`, `avl_read_sync2`, `avl_read_pulse`.

All seven reset to zero, matching the existing source phases. Core reset still
retains `avl_read_i`, `avl_read_accepted`, `avl_return_credits` and
`avl_return_phase`. Accepted memory transactions remain obligations across reset;
old responses drain without creating work for the new output epoch. Read-credit
arithmetic, copy retirement, the terminator and passive diagnostics are unchanged.

## Host validation

Use the pinned Menu checkout and the tool versions in
[FPGA development](fpga-development.md). All commands below are host-only. The
integration checker reads Menu files from the pinned Git object, applies the
current patch, and preserves generated sources, waveforms, logs and hashes.

```sh
python3 scripts/checks/check-fpga-scaler-reset.py "$MENU" \
  --artifacts-dir build/scaler-reset
python3 scripts/checks/check-fpga-scaler-request-formal.py \
  build/scaler-reset/source/sys/ascal.vhd --artifacts-dir build/request-formal
python3 scripts/checks/check-fpga-scaler-completion-formal.py "$MENU" \
  --artifacts-dir build/completion --report build/completion.json
python3 scripts/checks/check-fpga-terminator-boundary.py "$MENU" \
  --scheduler-artifacts build/completion --artifacts-dir build/boundary
python3 scripts/checks/check-fpga-causal-evidence.py --artifacts-dir build/causal
python3 scripts/checks/check-fpga-latch-integration.py "$MENU" --simulate
```

Run each command even when another fails, preserving individual exit statuses.
The complete integration and terminator checks currently fail on the separate
boundary defect described below. Do not convert that failure into a safety pass.

The complete-scaler regression uses GHDL's fixed-width netlist for the production
VHDL and the pinned `f2sdram_safe_terminator`, with syntax-only normalization for
Icarus. Unspecified synthesized storage receives a one-time zero power-up value;
explicit RTL initial values are preserved. No state is forced after power-up.
The geometry is 960x540 RGB565 scaled to 1920x1080, with production `run=1`.

The memory responder counts physical acceptances and returns independently of
DUT credits. Each acceptance creates 128 response beats. The checker compares
remaining physical words with retained production accounting at every memory
clock, checks that old responses stay in the drain, and verifies output credit
conservation using the real issue toggle and copy FSM retirement. Every successful
case must resume at least three accepted reads and three copies after reset.

The 72-case matrix includes both idle toggle phases, a no-reset control, requests
and acknowledgements in flight, copy completion and retirement, one and two
accepted reads, partial bursts, return gaps, acceptance during reset, final old
responses after release, stopped clocks, repeated resets, three additional clock
ratios/phases, and reset offsets around pipeline events. The same matrix runs
against a mutation removing exactly the seven assignments. The original idle
high-phase case must fail specifically with an uncharged read.

The request formal check extracts the production sender, synchronizer, pulse and
acknowledgement assignments and their reset values. Its environment may request
again after acknowledgement and may delay acceptance arbitrarily; independent
clock enables allow either domain to stop indefinitely. This compositional proof
checks transport conservation. Its abstract acceptance environment does not
prove the physical terminator or copy retirement. Those paths remain covered by
the complete-RTL matrix and the separate boundary checks.

The existing completion proof checks its exact-source functions, bounded safety,
induction and required covers. Its abstract pending-read counter and guarded
copy-retirement input exclude the original phantom-request path. A pass of that
check alone cannot establish closure of this incident.

## Remaining boundary failure

A wait-stalled request retained by the terminator can be accepted downstream
after core reset release while the scaler-facing request is gated off. The
independent ledger then contains 128 outstanding words while production
accounting contains zero. The complete-scaler `stage-12` case now exercises that
release window directly, alongside the existing formal counterexample and
observer detection regression.

This failure exists with and without the seven assignments. It is a separate
functional repair and prevents claiming universal reset safety. Passing the
other host checks does not authorize FPGA activation or replace fixed-seed
Quartus timing/CDC signoff and physical qualification.

## Recorded host result, 10 October 2026

The complete matrix passed 71 of 72 cases with the repair and 43 of 72 with the
seven assignments removed. All 28 newly passing cases failed before the repair;
no passing baseline case regressed. Both versions failed `stage-12` with physical
outstanding words equal to 128 and production words equal to zero.

The request transport passed 64-step bounded safety, temporal induction and its
64-step stopped-clock/high-phase-reset/recovery witness. The existing completion
proof passed its safety, induction and required covers. The production bridge
simulation and passive-observer checks passed. The terminator safety proof failed
and its separate detection regression passed, preserving the known failure.

These results close the reproduced toggle mechanism within the checked contract.
The overall host safety result remains failed because of the terminator defect.
No device operation or Quartus build was performed.
