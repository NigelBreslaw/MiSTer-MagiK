# Kernel scanout plugin assurance contract

Status: implementation contract. Passing the host and device checks does not
constitute a safety certification.

This document describes the qualified 5.15 profile and the separate
[Linux 6.18 production gates](#linux-618-production-gates). The development
provider does not relax the qualified profile's checks or establish production
qualification.

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
| KS-002 | One immutable 64-byte ABI v3 layout query | Rust layout tests; unknown-ioctl device test |
| KS-003 | Slot bases are `0x227e9000` and `0x22fd2000` | compile-time checks; userspace exact-layout validation |
| KS-004 | Both mappings are exactly 2,101,248 bytes | boundary tests; device mmap rejection matrix |
| KS-005 | Mappings are shared, read/write, non-executable and write-combined | source audit; device VMA/PTE inspection |
| KS-006 | Unsupported kernel/framebuffer platforms and occupied resource ranges fail before device registration | host source test; negative instrumented-kernel test |
| KS-007 | The module allocates nothing and performs no DMA, routing or presentation | source and binary denylist |
| KS-008 | A missing or rejected module preserves stock recovery; production hidden-slot admission fails closed | boot and launcher lifecycle test |
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

## Linux 6.18 production gates

The [fixed-window development provider](../mister/platform/kernel/scanout-618/README.md)
already implements the Main window, unchanged two-slot ABI v3, bounded mapping
diagnostic, resource lifecycle and mapping-policy checks. Its ordinary object
is activation-disabled; the usable trial is explicitly development-only.
Successful paired-kernel mappings and presentations do not promote it to
production or prove FPGA DDR-read quiescence.

Production qualification still requires the following gates, complete fault
and repeated-handoff evidence, physical output validation and transactional
packaging of exact reviewed artifacts. The qualified 5.15 platform stays
recoverable; KS-001/004/006/009 are not loosened by development evidence.

### Mapping-attribute acceptance gate

The intended ARMv7 type is Normal memory with inner/outer non-cacheable
attributes, using the kernel's write-combine mapping APIs. Every overlapping
alias must agree on memory type, cacheability and shareability. A name such as
`MEMREMAP_WT`, a userspace VMA label, or a visible picture does not prove this.

Before production activation, a separately reviewed, attended diagnostic must:

1. Bind its evidence to boot, kernel configuration, module, Main process and
   relevant mappings. Record the PFNs actually covered, not just virtual bases.
2. Inspect the effective translations for the stock driver's aperture, both
   module views and Main's new view, including relevant ARM memory-remap state.
   Use only supported, license-compatible kernel interfaces. If unavailable,
   report that limitation rather than bypassing exports or assuming equivalence.
3. Confirm Main's overlapping `/dev/mem` alias has been eliminated at a process
   startup boundary. Do not hot-swap under live Imlib objects or worker pointers.
4. Retain decoded type/cacheability/shareability evidence and check it against
   the reviewed kernel implementation. Do not infer all pages from one sample
   without proving the mapping construction is uniform over the full interval.

The development provider's per-mapping diagnostic and PFN/protection checks
exist, but do not by themselves establish agreement across every overlapping
alias. Further inspection remains separately reviewed, bounded and attended.
Barriers/cache maintenance cannot repair incompatible aliases.

### Identity and writer-exclusion acceptance gate

Removing `registered_fb[0]` must not silently remove its safety obligations.
Separate the checks that a module can enforce (platform, geometry, PFNs,
resource claim and mapping policy) from session checks owned by Main.

The exact supported replacement for independent framebuffer identity validation
is still unresolved. A boot-bound native config report and userspace
`FBIOGET_FSCREENINFO` are useful evidence, not a kernel-enforced identity proof.
The profile cannot be marked qualified until the revised trust boundary and its
failure handling are reviewed. Do not cast a file's private data, call hidden
symbols, or change the module's license declaration to gain access.

Main must close writer admission and drain already-admitted synchronous and
queued writes before transfer. Mode/VT changes precede final slot initialization;
final framebuffer metadata must exclude overlap with either slot. During the
lease, the stock driver's full-aperture clear and console/VT write paths must
also be excluded using supported stock-platform controls. Merely checking Main's
launcher flag or detecting a later mode change is insufficient.

Which stock controls exclude all those driver writes is a remaining design gate,
not an implemented guarantee. If they cannot do so, stop this design and revise
it within the no-kernel-fork constraint. A successful resource claim does not
revoke these writers on the tested configuration.

Likewise, ownership transfer requires the future token-matched FPGA drain
receipt. The existing route-at-vblank acknowledgement is insufficient. Neither
publisher exit nor elapsed time authorizes Main to reuse pages.
