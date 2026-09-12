# Stock Linux 6.18 production mapping contract

Status: proposed implementation contract, not an activated platform profile.
This refines [ownership ordering](kernel-scanout-ownership-protocol.md), not the
currently qualified [5.15 module contract](kernel-scanout-plugin-assurance.md).
No kernel fork, replacement latch, or `/dev/fb0` rendering path is introduced.

## Evidence boundary

The explicitly approved corrected probe succeeded on `6.18.38-MiSTer`, boot
`bde9063f-8476-45e0-92b0-3d76ea63ba25`. Both the attempt and independent follow-up
receipts report `claimed_and_released=true`, no result/cleanup error, and
`module_present=false`. The module SHA-256 was
`5e480a795b659b4daf3b116f0b6727d9e3e1b60ef01bd00cc5da845a4e265d6e`.
Local evidence is retained under ignored run directories
`20260909T231439Z-6ee7cf665c4d` and `20260909T231454Z-46e4107e5ec6`.

This proves that the probe executed its platform/PFN checks and could transiently
reserve the whole window at that time. It establishes neither persistent
ownership nor mapping attributes, CPU/FPGA quiescence or exact running-image
source identity. The earlier `ENODEV` receipt remains preserved. No new device
operation is required to document these historical results.

## Fixed interfaces

| Interface | Physical interval, end exclusive | Mapping selector and length |
| --- | --- | --- |
| Existing slot 0 | `[0x227e9000, 0x229ea000)` | Existing selector 0; 2,101,248 bytes |
| Existing slot 1 | `[0x22fd2000, 0x231d3000)` | Existing selector 8,294,400; 2,101,248 bytes |
| Proposed Main window | `[0x22000000, 0x237bb000)` | Selector 0; exactly 24,883,200 bytes |

- Preserve `/dev/mister-magik-scanout-slots`, its immutable 64-byte ABI v3 query,
  both selectors, RGB565 geometry and flags. Do not reinterpret its offsets.
- Add `/dev/mister-magik-main-window`, mode `0600`, owned by the same module.
  It accepts only its entire fixed window at offset zero. It has no read/write
  file-operation copy path and accepts no supplied physical address.
- Give Main a separate version-1, fixed-size 64-byte layout query: sixteen
  fixed-width `u32` words containing version, structure size, physical base,
  map length, attribute flags, page size, and ten zero reserved words. Assign
  and collision-check its ioctl number when introducing the UAPI header.
  Main checks all fields, including reserved words, before constructing pointers.
- Both devices require shared read/write, non-executable mappings. Reject
  partial, oversized, private, executable and unknown-selector requests.
  Preserve v3 error conventions; test later `mprotect` attempts as well as mmap.
- Reserve the full window once before registering either device. Slots are
  contained views, not additional overlapping resource requests. Reject valid
  RAM PFNs and resource conflicts. Roll back registration and reservation on
  any initialization failure; do not leave only one new interface published.
- Retain the claim while any mapping/file/module reference remains. Closing an
  fd while its VMA survives must not permit unload. No forced-unload recovery.

The layout ABI is not a writer lease. Root-only permissions are access control,
not isolation from privileged code or the FPGA. The provider remains a bounded
mapping mechanism: no allocator, presentation, ownership mailbox or DMA engine.

## Mapping-attribute acceptance gate

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

No new inspection module, page-table read, mapping, cache operation or device
load is authorized by this document. Its diagnostic mechanism remains to be
designed. Barriers/cache maintenance cannot repair incompatible aliases.

## Identity and writer-exclusion acceptance gate

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

## Logical follow-up commits and release gates

1. Define the Main UAPI and pure geometry/mapping-policy tests; preserve v3
   byte-for-byte. This is scaffolding, not a production profile activation.
2. Implement the provider's fixed views, resource lifecycle and 6.18 VMA APIs;
   test failed claims, partial registration, every rejection case, fd-close
   with surviving VMAs and normal unload. Keep unqualified delivery disabled.
3. Implement and approve the bounded attribute-inspection diagnostic and the
   independent identity/trust-boundary replacement. Retain actual target evidence.
4. Implement Main's fixed mapping and writer admission/drain, including supported
   exclusion of stock framebuffer writers. Test queued mode-write races.
5. Implement the separately versioned FPGA quiesce/arm protocol and prove delayed
   requests/returns, stale tokens, resets and pending route changes fail closed.
6. Integrate lifecycle transitions, test faults and repeated handoffs, verify
   physical output, then qualify and transactionally package exact artifacts.

Steps may have preparatory parallel work, but no production activation precedes
all acceptance gates. Changes to KS-001/004/006/009 remain proposed until that
qualification; the old profile and its checks are not loosened in advance.
