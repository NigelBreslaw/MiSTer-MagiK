# FPGA development playbook

Use this playbook only for changes to the FPGA proof model, cache identity,
timing constraints, or Quartus interpretation. Release qualification remains
governed by `fpga-latch-release.md`.

## Supported toolchain

The established Apple Silicon toolchain is GHDL 6.0.0 with its LLVM backend,
Yosys 0.68, Icarus Verilog 13.0, and pinned Quartus Prime Lite 17.0 Build 595
inside Apple `container`. Use the repository interfaces; do not substitute an
untracked binary or assemble a different proof command and call it equivalent.

The only synthesis entrypoints are:

```text
QUARTUS_ACCEPT_EULA=1 scripts/agent fpga setup
scripts/agent fpga signoff
```

The default reusable root is `build/fpga-local-apple/`. A stable absolute
`MISTER_FPGA_LOCAL_ROOT` may be shared by worktrees. Completed stock,
pre-observer, and patched variants are bound to their source inputs, pinned
revisions, Quartus seed and date, preparation-script identity, reports,
metadata, and RBF hashes. Hidden `.VARIANT.building` directories are incomplete
staging and never cache hits. Do not edit cached reports, copy evidence between
variants, or use `--rebuild` speculatively.

Signoff builds `refs/heads/main^{commit}`. Commit the frozen candidate and move
local `main` before synthesis. Preserve the root commit and patched-source,
RBF, metadata, and delta-report SHA-256 values as one identity.

## Development sequence

1. State the causal failure and invariants; distinguish known RTL causes from
   symptoms requiring physical proof.
2. Freeze interfaces and numerical gates. Avoid diagnostic opcodes or unrelated
   output logic during closure.
3. Compile and structurally bind the actual production VHDL.
4. Exercise reset, stopped clocks, simultaneous events, queue limits, and
   wraparound in the exact-topology simulations.
5. Run exact-source bounded proof and required non-vacuity covers.
6. Run temporal induction using production-derived range, pipeline, and
   coherence invariants as assertions, never assumptions that hide a failure.
7. Freeze the integrated commit; formal signoff and cached Quartus may then run
   concurrently.
8. On a real failure, fix the smallest causal cone, repeat cheap gates, and
   rebuild only the invalidated variant.
9. Install only through the attended rollback-capable Dev transaction.
10. Use output-rate physical capture for smoke and qualification.

## Formal-model rules

- Compile patched production `ascal.vhd` and its package. A narrow formal DUT
  may call exact transition functions, but structural binding to production
  scheduler sites remains a separate required proof.
- Build the responder scoreboard from accepted Avalon transactions and returned
  beats, never from the DUT credit counter.
- Keep `waitrequest`, return gaps, clock phase, and stops arbitrary unless the
  integrated topology proves a narrower contract.
- Model asynchronous reset assertion and synchronous release in each domain.
- Distinguish asserted, accepted, outstanding, returned, queued, pulsed, and
  consumed work across reset.
- Drive covers from real acceptance, return, reset, vertical-sync, and clock
  events rather than fixed cycle numbers.
- Classify solver results before editing RTL: reachable counterexample,
  unreachable cover, impossible induction start, and production defect require
  different responses.

## Quartus interpretation

Declare gates before the first build. The scaler-repair baseline used seed 2,
setup slack at least 0.428 ns, hold slack at least 0.200 ns, zero TNS, no more
than 0.150 ns matched-baseline setup degradation, all 158 constrained
relationships, at most 150 additional ALMs and 96 registers, unchanged
RAM/DSP/PLL identity, and combined synchronizer MTBF of at least `10^12`
device-hours.

Validate exact forward and reverse CDC endpoints and net-delay rows, not only
global counts. Tool recognition may expose a legacy synchronizer without
creating a new physical chain. Never rescue failure with a seed sweep, waiver,
false path, LogicLock, fitter change, or unrelated reset controller.

A local signoff pass makes an RBF eligible only for an attended Dev install.
Production requires CI reconstruction and the physical frame-evidence, stress,
long-latch, and canary gates. Fail closed on black, stale, partial, banded,
corrupt, or indefinitely blank physical output.
