# Seed-2 FPGA video diagnostics retirement evidence

On 2026-08-12, three committed revisions of the passive FPGA video diagnostics
were synthesized locally with Quartus 17.0.0 Build 595 and the fixed fitter
seed 2. All three completed builds were rejected by the unchanged delta
checker. None of their RBFs was installed or deployed.

This record closes the timing-patch investigation. The real HDMI FPLL status
ingress remains valid, but the wide three-clock snapshot architecture is being
retired rather than patched again.

## Checkpoints

### Original seed-2 observer at `7bb59b65`

- Final setup slack: 0.075 ns
- Final hold slack: 0.231 ns
- Final TNS: 0
- Baseline/final unconstrained output paths: 158/158
- Resource delta: +930 ALMs, +1433 registers
- Diagnostic CDC minimum skew/net-delay slack: 4.039/3.867 ns
- Diagnostic paths at 0.153/0.188 ns:
  `reference_lines[5]` to `fault_flags[4]`/`fault_flags[7]`

The RBF failed the minimum setup-slack gate. GitHub workflow run
`31629415639` built the pushed `7bb59b65` inputs and passed its matched delta,
but publication was disabled. That run does not supersede the deterministic
local fixed-seed failure or qualify the later unpushed experiments.

### Pipelined output capture at `6b4d2e9d`

This revision includes the control cleanup at `980e6905`.

- Final setup slack: 0.052 ns
- Final hold slack: 0.242 ns
- Final TNS: 0
- Baseline/final unconstrained output paths: 158/158
- Resource delta: +937 ALMs, +1467 registers
- Diagnostic CDC minimum skew/net-delay slack: 4.384/3.752 ns

The pipeline removed the direct 0.153/0.188 ns diagnostic paths. Its worst
diagnostic path was 0.543 ns from `frame_period` to `snapshot_generation`, but
global timing still failed on an unrelated legacy scaler path. The completed
local cache for this checkpoint was subsequently superseded, so no durable RBF
hash is claimed here.

### Request-recognition capture at `b43ef0ea`

- Final setup slack: 0.285 ns
- Final hold slack: -0.293 ns
- Final TNS magnitude: 0.293 ns
- Baseline/final unconstrained output paths: 158/160
- Resource delta: +956 ALMs, +1445 registers
- Diagnostic CDC minimum skew/net-delay slack: 2.925/3.902 ns
- Rejected RBF SHA-256:
  `1be522aa949bd54c03323f8f79d6556aeffbfa6d65b3e5ede0c72561238239f5`

The exact hold violation is a diagnostic bundled-data crossing from
`mister_magik_video_diagnostics_control|expected_route_epoch[12]` in
`clk_sys` to `mister_magik_video_diagnostics_output|route_epoch[12]` in the
HDMI domain. The two added unconstrained rows are fitter-created duplicates of
`emu|act_cnt[7]` routed to `LED[0]` and `LED[4]`.

The retained ignored local evidence is under
`build/fpga-local-apple/signoff/`, including
`quartus-delta-signoff.tsv`, the patched timing reports, metadata, and the
rejected RBF. The metadata binds the RBF to `b43ef0ea`, seed 2, and the hash
above.

## Engineering conclusion

The attempts proved that local pipeline changes could repair individual
diagnostic paths, but the architecture remained too large and physically
sensitive, and the final experiment exposed a real multibit diagnostic CDC
hold failure. The observer cost remained approximately 930-956 ALMs and
1433-1467 registers throughout.

No timing threshold, seed, exception, latch source, generated PLL interface,
or functional video cone will be changed to accommodate it. The replacement
will preserve the proven `reconfig_from_pll[16]` real-FPLL ingress and use a
small lock recorder plus a separately qualified final-HDMI evidence recorder,
with no wide fabric payload crossing.
