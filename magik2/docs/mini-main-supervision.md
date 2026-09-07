# Mini mapped-input correction

Main remains unchanged. No raw joystick fallback and no input lease.

## Implementation checklist

- [x] Reuse the real app's existing Main-managed lifecycle from `f511dad29`.
- [x] Parameterize the owned artifact; preserve the marked Dev launcher environment block.
- [x] Require `main-managed-mini-v1`, so an older compatible service cannot silently use suspended launch.
- [x] Require Main's active child PID, input capability and FPGA ownership for readiness.
- [x] Inherit Main's display contracts instead of querying its busy startup command lane.
- [x] Complete ready-v3 using two actually confirmed alternating nonblank RGB565 frames.
- [x] Record physical input edges independently of synthetic test actions.
- [x] Deploy and confirm Main reports LauncherActive with Mini as its child.
- [x] Pass consumer smoke and capture; user confirms all joystick movement works correctly.

The installed service advertised `main-managed-magik`, but this consumer checkout
predated that implementation. The old Mini path suspended Main and directly
spawned Mini; an open proxy descriptor was therefore not proof of active routing.
The existing real-app fix is reused here instead of changing Main's input policy.

Startup presentations are outside motion measurement windows. Runtime controller
events still use Main's mappings and the existing neutral/release handling.
No flips, catalog integration, Main binary changes or platform delivery belong here.

## Verification

- Native lifecycle tests: 2 passed; process/artifact readiness and superseded-upload tests: 2 passed.
- Runtime ready-v3 receipt test: 1 passed. Mini control tests: 2 passed.
- Agent and Mini Clippy passed; Rust LSP reported no diagnostics for the new runtime helper and Mini.
- Deployment capability Python tests: 4 passed. Consumer display/motion validation unit tests: 9 passed.
- Deploy: `build/magik2-results/20260907T205835Z-0226faca5b1f`.
- Main remained PID 581, generation 9359; reports LauncherActive, ready, FPGA owner magik.
- Smoke/native capture: `build/magik2-results/20260907T210355Z-f21739e0bc86`, 1 passed in 27.60s.
- First smoke exposed a test-only lifecycle-lane contention from calling device-status
  inside the active session; replaced with the read-only native readiness gate.
- Physical observation: `build/magik2-results/20260907T210436Z-f358924aa7d5`;
  initial Mini PID 7095, zero physical edges, Arcade, no runtime error. User subsequently confirmed all movement correct.

Captures/logs remain untracked. The Main input desync counter was 1 at the first
post-deployment status and unchanged at the next observation; this is not a zero-desync qualification.
