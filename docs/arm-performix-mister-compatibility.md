# Arm Performix and MiSTer compatibility

Status: blocked by the target platform, verified 2026-08-06.

## Conclusion

Arm Performix cannot currently profile MiSTer MagiK on MiSTer's Cortex-A9
runtime. Arm documents Performix for Linux on Arm Neoverse platforms. MiSTer is
an ARMv7 Cortex-A9 platform, outside that supported target set.

The repository's direct `perf_event_open` probe independently confirms that the
installed MiSTer kernel does not expose the hardware PMU event required for this
work. Opening the cycle leader returns `EINVAL` before any workload runs.
Consequently there is no authoritative cycles, instructions, L1D, or branch
baseline, and no PMU-guided optimization claim can be made.

Arm product source:
<https://developer.arm.com/servers-and-cloud-computing/arm-performix>

## Reproduction

The exact installed Dev runtime was built from commit
`bc62e2ddc91e256af41c9e9dc6bc5258e29b2c2a` and delivered through
`scripts/agent deliver`. The authoritative entry point was then run through:

```text
scripts/agent benchmark pmu-profile
```

The probe attempted a bounded compatibility matrix:

1. event IDs plus enabled/running time, calling thread on any CPU;
2. ordered group values, calling thread on any CPU;
3. ordered values without the `perf_event_open` close-on-exec flag;
4. ordered values without the hypervisor-exclusion bit; and
5. ordered values bound to the calling thread's current CPU.

Every variant failed while opening `PERF_COUNT_HW_CPU_CYCLES`:

```json
{
  "stage": "open-event",
  "event": "cycles",
  "errno": 22,
  "message": "Invalid argument (os error 22)"
}
```

The suite rejects this result. It does not convert missing events into zero
counters, run the remaining workloads, or emit a passing baseline.

## Resume gate

Do not resume the PMU baseline or the dependent optimization commits until all
of these conditions hold:

- the MiSTer kernel exposes a hardware PMU event source for Cortex-A9;
- `pmu-probe` reports non-zero cycles and instructions with its collection mode;
- three independent `scripts/agent benchmark pmu-profile` runs pass; and
- the normal screensaver and search qualification gates still pass.

Enabling the Cortex-A9 PMU is platform work, not an application workaround. It
requires an explicit kernel/configuration investigation in the maintained
Main_MiSTer platform scope. User-space CP15 access, `/dev/mem` counter scraping,
silent software-counter substitution, and unqualified raw event mappings are
not acceptable substitutes.

The application instrumentation is dormant unless `MISTER_PMU_PROFILE=1`, so
retaining it does not change normal rendering, catalog, or search behavior.
