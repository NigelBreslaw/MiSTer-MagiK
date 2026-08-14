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

Commands `0x60` through `0x67` are therefore unsupported by the implemented
repair candidate. Decoders remain only for already-qualified historical
RBFs and rollback evidence. Explicit unsupported diagnostics are not an
activation failure when exact manifest/RBF identity and the unchanged latch-v5
contract pass.

The complete attempt history and retained measurements are in
[FPGA video diagnostics: two attempted designs and their retirement](../history/2026-08-14-fpga-video-diagnostics-design-attempts.md).
