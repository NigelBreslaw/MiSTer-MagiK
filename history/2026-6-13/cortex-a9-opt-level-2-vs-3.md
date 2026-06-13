# Cortex-A9 opt-level=2 vs opt-level=3 fat LTO trial

Date: 2026-06-13

Question: does the MiSTer FPGA's ARM Cortex-A9 perform better when the full
fat-LTO device build uses LLVM/Rust `opt-level=2` instead of `opt-level=3`?

Test shape:

- Baseline: `release-device` (`opt-level=3`, fat LTO, `codegen-units=1`,
  `-C target-cpu=cortex-a9 -C target-feature=+neon`).
- Experiment: temporary `release-device-opt2`, identical except
  `opt-level=2`.
- UI scope: `arcade`.
- Device scenarios: `held-scroll` and `turbo-hold`, 30 seconds each.
- Tooling: `scripts/bench-toolchain.sh`, deployed each binary to
  `/media/fat/mister-magik/mister-magik-fb` and ran the real `ui arcade` path.

Results:

| Build | Scenario | Size | Render | Present | FPS | CPU mean |
|---|---:|---:|---:|---:|---:|---:|
| opt3 fat LTO | held-scroll | 5,873,444 B | 109us | 2754us | 60 | 47% |
| opt2 fat LTO | held-scroll | 5,304,188 B | 114us | 2855us | 60 | 49% |
| opt3 fat LTO | turbo-hold | 5,873,444 B | 155us | 2896us | 60 | 56% |
| opt2 fat LTO | turbo-hold | 5,304,188 B | 150us | 2958us | 60 | 59% |

Conclusion:

`opt-level=2` produced a smaller binary, but it did not improve the real arcade
runtime. It was slightly worse on framebuffer-present time and CPU in both
scenarios, with only a tiny render-time win in `turbo-hold`. Keep the production
`release-device` profile on `opt-level=3` unless a future workload shows a
clearer regression.

Benchmark caveat: the harness appended valid timing rows, but its mid-run
framebuffer PNG capture failed in all four runs, so the visual gate marked the
rows `visual_ok=no`. The UI itself ran at 60fps in every case; the failure was
capture evidence, not runtime startup.
