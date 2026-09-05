# Arm Performix and MiSTer compatibility

Status: Performix incompatible; Cortex-A9 PMU and Streamline paths validated
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

The repository's diagnostic isolated the earlier `EINVAL`: the Cortex-A9 PMU
accepts disabled and grouped events, but rejects privilege filtering through
`exclude_kernel`. The counter group therefore measures the calling thread in
both user and kernel mode, which matches this PMUv1 implementation's supported
scope. Failed exclusion requests remain in the diagnostic evidence rather than
being mistaken for an absent PMU.

Arm product source:
<https://developer.arm.com/servers-and-cloud-computing/arm-performix>

Streamline and gator sources:
<https://developer.arm.com/tools-and-software/streamline-performance-analyzer>
<https://github.com/ARM-software/gator>

## Reproduction

The exact installed Dev runtime is delivered through `scripts/agent deliver platform`.
The authoritative counter entry point is then run through:

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
diagnostic now starts with leader-only cycle requests that isolate the disabled,
group-format, and privilege-exclusion attributes. It then opens calling-thread
groups without unsupported privilege filters.

The former group variants all failed while opening
`PERF_COUNT_HW_CPU_CYCLES` because each requested `exclude_kernel`:

```json
{
  "stage": "open-event",
  "event": "cycles",
  "errno": 22,
  "message": "Invalid argument (os error 22)"
}
```

The corrected group then exposed one further Linux 4.19 compatibility boundary:
`PERF_EVENT_IOC_ID` returns `ENOTTY`. The collector now falls back to ordered
group reads only for `EINVAL`, `ENOTTY`, or `EOPNOTSUPP`; other failures remain
fatal. The resulting suite reports real Cortex-A9 cycles, instructions, L1D
accesses/refills, branches, and branch mispredicts.

## Bounded Streamline capture

The repository deliberately does not download or redistribute Arm's prebuilt
`gatord`, or accept an Arm product EULA. Supply an audited ARMv7 hard-float
binary explicitly:

```text
MISTER_GATORD_PATH=/absolute/path/to/gatord scripts/agent benchmark streamline
```

Arm Performance Studio's macOS installation includes a statically linked ARMv7
target `gatord`. The workflow reads that target binary from the host
installation, hashes it, uploads it to volatile `/tmp` storage, and executes it
on the MiSTer. It is not part of the MagiK ARM application build. MagiK and any
source-built `gatord` remain Apple-container builds; the supplied target daemon
does not need recompilation.

The typed workflow is Dev-only. It uploads to `/tmp`, captures the fixed
`pmu-profile screensaver` workload for at most ten seconds, uses the low sample
rate, includes kernel execution because this PMUv1 rejects privilege exclusion,
and disables call-stack unwinding. Every capture explicitly programs the
Cortex-A9 cycle counter plus raw events `0x68`, `0x74`, `0x8c`, `0x61`, `0x04`,
and `0x03`; the app workload selects the matching runtime counter set. It
retrieves both an extracted
`mister-magik.apc` directory and its SHA-256-verified archive, then removes the
remote daemon and capture. Cleanup may terminate only the PID recorded by this
capture and only while `/proc/PID/exe` still resolves to the uploaded daemon.
The orientation variant temporarily suspends the ordinary launcher, captures
the exact installed Dev launcher running the fixed six-leg Settings route, and
then restores the ordinary launcher while preserving the current display mode.

The dedicated launcher scheduler pass is:

```text
MISTER_GATORD_PATH=/absolute/path/to/gatord scripts/agent benchmark launcher-response-streamline
```

It records the production Main proxy → launcher → kernel → latch path
system-wide while the fixed 1920×1200p60 Computers route runs at 200, 300, 400,
and 600 ms. App-only collection is deliberately unsuitable because the launcher
restarts between schedules and the scheduling evidence spans more than one
process. Capture is limited to 120 seconds, uses the low sampling rate, includes
kernel execution, disables call-stack unwinding, and always restores the prior
display mode and ordinary launcher. The report records the exact daemon version,
daemon hash, archive hash, boot identity, and route results; the repository does
not copy or redistribute the daemon. System-wide collection requires tracefs.
If tracefs is not already mounted, the bounded workflow mounts it at
`/sys/kernel/tracing`, records the exact `/proc/mounts` entry it owns under the
volatile capture root, and unmounts only while that entry still matches during
cleanup. It never persists the mount in boot configuration.
The host keeps the collector's SSH channel open for the capture lifetime while
an independent channel drives the input route; `gatord` is never orphaned into
an unowned remote session.

Optional headless analysis is opt-in and never launches the Streamline GUI:

```text
MISTER_GATORD_PATH=/absolute/path/to/gatord \
MISTER_STREAMLINE_ANALYZER_PATH=/absolute/path/to/sl-analyze \
scripts/agent benchmark streamline
```

The analyzer path must be absolute and name a non-empty regular file. The host
pre-creates `streamline-analysis/`, passes it with explicit `-o`, redirects both
streams to retained log files, and enforces a 120-second timeout. It never uses
`--no-output`. A successful run must produce CSV output and a structured
analysis summary; any analyzer failure invalidates the benchmark without
changing or opening the original APC in the GUI.

The combined diagnostic command uses the same explicit daemon and makes this
system timeline a required fifth arm rather than an optional add-on:

```text
MISTER_GATORD_PATH=/absolute/path/to/gatord scripts/agent benchmark launcher-response-attribution
```

The audited source contract is Arm gator commit
`f0774012f36dbdb543e082d3e14ca9db20d0432d` (gator 9.7.2). Its maintained
`build-linux.sh -p arm-glibc` profile targets ARMv7 Linux hard-float; the
`arm-musl` profile is the static alternative. The pinned source does not build
unchanged with that profile: its generated `-I daemon` precedes the C library
headers and shadows `<time.h>`, and its pinned GPU-info submodule exports the
`libarmgpuinfo` namespace while the daemon expects `libgpuinfo`. The audited
device capture used generated-build-only `-idirafter daemon` and
`-Dlibgpuinfo=libarmgpuinfo` workarounds. The source checkout remained
read-only; these boundaries must be re-audited before changing the pin.

The dated capture hashes and optimization measurements remain in Git history.
They establish provenance for that run, not qualification of a current candidate.

## Resume gate

The PMU baseline and dependent optimization may proceed only while all of these
conditions hold:

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
