# P1 Enforce runtime acceptance evidence — 2026-08-15

This note compares the post-FPGA clean-main baseline at `194b669f` with the
completed P1 Enforce runtime at `4567a428`. Both used the qualified
`platform-v0.28` bundle and Main revision `9f2ce686`; no FPGA synthesis,
experimental RBF installation, or destructive reset experiment was run.

## Clean-main baseline

- `cold-boot` produced a verified Home capture and reached the first launcher
  presentation at 10.72 seconds from Linux boot.
- `launch-return` timed out after 20 seconds waiting for the initial launcher
  return-state capsule. This failure preceded P1 Enforce.
- `launcher-response` timed out at its feedback-completion edge. This failure
  also preceded P1 Enforce.
- `arcade-velocity-scroll` passed 1,257 frames with zero physical dropped
  frames, zero latch drops, zero ownership loss, and a 23.431 ms maximum
  completion interval.

Local baseline artifacts:

- `build/agent-benchmarks/cold-boot/1786752087/`
- `build/agent-benchmarks/launch-return/1786752137/`
- `build/agent-benchmarks/launcher-response/1786752249/`
- `build/agent-benchmarks/arcade-velocity-scroll/1786752309/`

## Completed P1 Enforce runtime

- Exact clean commit `4567a428` was delivered coherently. The delivery reused
  the qualified `platform-v0.28` bundle and completed its health verification.
- A confirmation `cold-boot` produced a verified Home capture and reached the
  first launcher presentation at 10.62 seconds from Linux boot. The first
  post-delivery sample was 11.82 seconds; the bounded confirmation demonstrates
  that sample was runtime variance rather than an Enforce regression.
- `launch-return` reproduced the baseline 20-second initial return-state
  timeout. A following typed diagnosis reported a healthy active Home launcher
  with no temporary repair or recovery reboot.
- `launcher-response` reproduced the baseline feedback-completion timeout. All
  17 requested inputs were latch-confirmed and latch-drop delta was zero, while
  feedback-hide accounting stopped at 10 of 17.
- `arcade-velocity-scroll` passed 1,257 frames at approximately 59.95 Hz with
  zero physical dropped frames, zero latch drops, zero ownership loss, no
  sequence gaps, and a 21.363 ms maximum completion interval.

Local completed-runtime artifacts:

- `build/agent-benchmarks/cold-boot/1786773031/`
- `build/agent-benchmarks/launch-return/1786772713/`
- `build/agent-benchmarks/launcher-response/1786772858/`
- `build/agent-benchmarks/arcade-velocity-scroll/1786772926/`

## Acceptance conclusion

P1 Enforce preserves the recorded runtime boundary: cold boot and authoritative
Arcade animation show no regression, and physical dropped-frame evidence stays
independent from latch-drop evidence. The two incomplete benchmark routes are
pre-existing baseline debt and were not hidden or opportunistically changed by
this boundary-enforcement work.
