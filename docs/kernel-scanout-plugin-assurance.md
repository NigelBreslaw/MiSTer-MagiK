# Kernel scanout plugin assurance contract

Status: implementation contract. Passing the host and device checks does not
constitute a safety certification.

## Sole responsibility

On the pinned `5.15.1-MiSTer` platform, `mister_magik_scanout_slots` exposes
exactly two FPGA scanout regions associated with the framebuffer pipeline as
root-only, shared, non-executable, write-combined mappings. It does not allocate
memory or control presentation.

The stock DT resource describes only the visible 8 MiB framebuffer aperture;
it does not claim both hidden slots. Semantic ownership therefore comes from
the reviewed kernel/driver/DT/Main/RBF platform contract. Build, deployment and
boot checks prove the complete artifact fingerprint. At runtime the module
proves the qualified kernel/machine/framebuffer subset and reserves both
complete ranges exclusively before registering its device; those reservations
reject System RAM and occupied resource ranges.

The FPGA manifest pins the reviewed MagiK/Menu/patch/RTL identities separately
from the builder commit and fixes Menu's embedded `BUILD_DATE` to `260711`.
Repeated Quartus builds therefore use identical logic inputs instead of the
wall clock.

| Requirement | Contract | Verification |
| --- | --- | --- |
| KS-001 | One device, `/dev/mister-magik-scanout-slots`, mode `0600` | source/binary audit; device permissions |
| KS-002 | One immutable 64-byte ABI v1 layout query | Rust layout tests; unknown-ioctl device test |
| KS-003 | Slot bases are `0x227e9000` and `0x22fd2000` | compile-time checks; userspace exact-layout validation |
| KS-004 | Both mappings are exactly 1,040,384 bytes | boundary tests; device mmap rejection matrix |
| KS-005 | Mappings are shared, read/write, non-executable and write-combined | source audit; device VMA/PTE inspection |
| KS-006 | Unsupported kernel/framebuffer platforms and occupied resource ranges fail before device registration | host source test; negative instrumented-kernel test |
| KS-007 | The module allocates nothing and performs no DMA, routing or presentation | source and binary denylist |
| KS-008 | A missing or rejected module leaves `/dev/fb0` available as fallback | boot and launcher lifecycle test |
| KS-009 | Module identity is tied to repository source, platform contract, kernel/driver/DT config, UAPI, RBF and toolchain evidence | build provenance and deploy verification |

Mapping policy failures (unknown selector, partial or oversized length,
`MAP_PRIVATE`, missing read/write access, or executable access) return `EINVAL`.
Unknown ioctls return `ENOTTY`; a bad userspace layout pointer returns `EFAULT`;
an already claimed physical range prevents module load with `EBUSY`.

## Forbidden production behavior

The module must not contain DMA allocation/synchronization, cacheable aliases,
mailboxes, ownership or fence state, routing/posting, interrupts, timers,
workqueues, debugfs, sysfs, procfs, direct `/dev/mem` access, or compatibility
devices from `plugin-probe` and `mister-magik-scanout` experiments.

## Review and evidence

Changes are reviewed independently for repository standards and this contract.
Release evidence records the source revision, kernel revision/config, compiler,
UAPI hash, module hash, `modinfo`, imported symbols, host checks, deterministic
device rejection tests, lifecycle results, and HDMI evidence. Instrumented
kernel, sanitizer, model-checking and short QEMU fuzzing results remain explicit
release gates only after their workflows and retained reports exist.
