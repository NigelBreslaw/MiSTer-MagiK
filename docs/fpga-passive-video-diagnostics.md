# Passive FPGA HDMI evidence (retired production design)

The wide and staged FPGA video observers are retired from production. Their
field evidence was decisive, but every expanded implementation made the dense
legacy scaler physically sensitive and failed at least one fixed qualification
gate.

The later repair-only two-bit Gray candidate is also not qualified. With every
diagnostic opcode removed it still measured `0.072 ns` setup and `0.190 ns`
hold, below the required `0.428 ns` and `0.200 ns`. It must not be installed or
published.

Current implementation and qualification policy is
[FPGA scaler return recovery design](fpga-scaler-return-recovery.md). It selects
a queued one-bit request/acknowledgement repair which preserves the legacy
HDMI-side completion cone and keeps all FPGA observer commands out of the
production RBF.

The production repair candidate continues to expose no observer. A subsequent
experimental attribution candidate adds only `0x67`,
`scaler-scheduler-state-v1`. It exports one coherent scheduler-state word and
does not tap pixels, PLL state, routes, addresses, or final-output logic.
Commands `0x60` through `0x66` remain unsupported. Latch-v5 and capabilities
`0x03ff` remain unchanged, and the record always reports sink visibility as
unobserved.

`0x67` is diagnostic evidence, not a recovery command or production release
requirement. It has no write, clear, arm, reset, or freeze operation. Three
CRC-valid, identical coherent records are required before the host classifies
completion backlog, idle credit-accounting stall, or continuing scheduler
progress.

That experiment subsequently reproduced physical black with nine identical
coherent `0x0da3` samples. It ruled out the completion queue and the specified
credit-accounting stall for that occurrence, so the scheduler observer is
retired from the next candidate.

The current disposable experiment is
[Experimental raw-scaler frame-integrity diagnostic](fpga-raw-scaler-diagnostic.md).
It reuses only `0x67` with schema 3 and replaces the retired RGB/activity taps
with a CE/DE/HS/VS-only stable-baseline and sticky first-mismatch recorder.

The complete attempt history and retained measurements are in
[FPGA video diagnostics: two attempted designs and their retirement](../history/2026-08-14-fpga-video-diagnostics-design-attempts.md).
