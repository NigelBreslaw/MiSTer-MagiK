# Final-output activity FPGA signoff evidence

Date: 2026-08-13

The first fixed-seed-2 local signoff of the final registered HDMI output
activity recorder used committed HEAD `bb33351a` with FPGA implementation commit
`de0024fb`. The typed `scripts/agent fpga signoff` workflow rebuilt only the
patched variant and retained its reports and rejected RBF in the local signoff
cache.

The candidate was rejected and was not installed:

- setup slack: 0.164 ns, below the unchanged 0.20 ns floor;
- hold slack: 0.242 ns;
- TNS: zero;
- unconstrained output paths: 158, equal to the pinned pre-observer build;
- calculable synchronizer chains: baseline 5, candidate 9, the expected four-chain increase;
- resource delta: 113 ALMs and 156 registers, no block-memory or DSP increase;
- observer hierarchy: 104.7 ALMs and 123 fitted registers.

The worst setup path was unrelated scaler logic,
`ascal|o_hcpt[5] -> ascal|o_vcpt[7]`, with five logic levels. No diagnostic node
was on the failing path. The register failure was attributable to the observer's
three eight-bit destination epochs and separate streamed snapshot state.

The bounded next correction keeps the same seed, timing gates, three independent
toggle CDCs, `0x61` word layout, and passive final-output taps. It changes the
epochs to four-bit modulo counters, shortens the host sample window to about 50
ms, uses each counter LSB as its last-seen toggle state, and packs all activity
counters and flags into the existing 16-bit snapshot register. No scaler,
placement, timing constraint, latch, video, PLL, or reset logic is changed.
