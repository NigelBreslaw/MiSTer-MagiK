# FPGA diagnostic decode timing experiment

This note retains the fixed-seed result for the bounded diagnostic command
decode and event-counter experiment at `0382ac3b`. The candidate combined:

- `85687a72` — shared decoding of evidence commands `0x60` through `0x66`;
- `1fe989c0` — shortened modulo event-counter carry chains;
- `0382ac3b` — retained event predicates needed by collision detection.

The canonical local Apple signoff used Quartus 17, seed 2, and four processors
for all matched variants. Stock and pinned pre-observer variants were cache
hits; only the patched variant was synthesized.

The checker rejected the candidate:

- setup slack: `0.252 ns` (required `0.428 ns`);
- hold slack: `0.241 ns`;
- TNS: `0`;
- unconstrained output paths: `158`, equal to the pinned baseline;
- fitted delta: `+316 ALMs`, `+299 registers`, no RAM or DSP delta;
- synchronizer chains: `377`, equal to the pinned baseline;
- calculable synchronizers: `29` versus baseline `5`;
- two scaler-completion Gray net-delay paths applied with minimum slack
  `8.182 ns`.

The worst setup path was wholly inside legacy `ascal`, from
`o_vcpt_pre3[1]` to `o_fload[0]`. Reducing local diagnostic ALMs did not
recover global placement margin: the prior restored candidate measured
`0.400 ns` setup with `+338 ALMs` and `+270 registers`, while this experiment
fell to `0.252 ns` despite using fewer ALMs. This is evidence that the merged
seven-command latch transport and broad multi-domain observation boundary is
the wrong architecture, not evidence for further Boolean or seed iteration.

No RBF from this experiment was installed or release-qualified. The three
experimental commits remain in Git ancestry and are retired by explicit
reverts before the independent-sidecar redesign.
