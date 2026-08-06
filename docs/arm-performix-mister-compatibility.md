# Arm Performix and MiSTer compatibility

Status: Performix incompatible; Cortex-A9 PMU investigation active, corrected
2026-08-06.

## Conclusion

Arm Performix cannot currently profile MiSTer MagiK on MiSTer's Cortex-A9
runtime. Arm documents Performix for Linux on Arm Neoverse platforms. MiSTer is
an ARMv7 Cortex-A9 platform, outside that supported target set.

Arm Streamline is the relevant Arm tool for this platform. Arm supports software
profiling on A-profile CPUs and ships an `armv7a-hardfloat` gator daemon for
embedded Linux targets.

The installed MiSTer kernel does register its Cortex-A9 PMU. Its boot log reports
`armv7_cortex_a9 PMU driver, 7 counters available`; the pinned kernel enables
perf events, hardware perf events, profiling, high-resolution timers, tracing,
and `CONFIG_ARM_PMU`, while the SoCFPGA device tree supplies both PMU interrupts.

The repository's direct `perf_event_open` probe still returns `EINVAL` while
opening its cycle event. That result blocks authoritative cycles, instructions,
L1D, and branch measurements, but it does not establish that the kernel lacks a
PMU. The probe now records the kernel event sources, perf security setting, a
minimal cycle request, and every grouped compatibility request so the remaining
userspace or runtime compatibility issue can be isolated.

Arm product source:
<https://developer.arm.com/servers-and-cloud-computing/arm-performix>

Streamline and gator sources:
<https://developer.arm.com/tools-and-software/streamline-performance-analyzer>
<https://github.com/ARM-software/gator>

## Reproduction

The exact installed Dev runtime was built from commit
`bc62e2ddc91e256af41c9e9dc6bc5258e29b2c2a` and delivered through
`scripts/agent deliver`. The authoritative entry point was then run through:

```text
scripts/agent benchmark pmu-profile
```

The original probe attempted a bounded compatibility matrix:

1. event IDs plus enabled/running time, calling thread on any CPU;
2. ordered group values, calling thread on any CPU;
3. ordered values without the `perf_event_open` close-on-exec flag;
4. ordered values without the hypervisor-exclusion bit; and
5. ordered values bound to the calling thread's current CPU.

The fifth request did not actually pin the thread and has been removed. The
diagnostic now starts with a leader-only cycle event having no grouping, read
format, exclusion, or disabled flags, followed by the four calling-thread group
requests.

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

- the runtime reports `ARMv7_Cortex_A9` in its perf event sources;
- `pmu-probe` reports non-zero cycles and instructions with its collection mode;
- three independent `scripts/agent benchmark pmu-profile` runs pass; and
- the normal screensaver and search qualification gates still pass.

The kernel and device-tree prerequisites are already present in the maintained
`Linux-Kernel_MiSTer` source. `Main_MiSTer` is a userspace binary and is not the
owner of this facility. User-space CP15 access, `/dev/mem` counter scraping,
silent software-counter substitution, and unqualified raw event mappings remain
unacceptable substitutes.

The application instrumentation is dormant unless `MISTER_PMU_PROFILE=1`, so
retaining it does not change normal rendering, catalog, or search behavior.
