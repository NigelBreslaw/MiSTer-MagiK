# Stock-6.18 provider core (activation disabled)

This implements the fixed-window mechanism from
`docs/kernel-scanout-618-production-contract.md`, not a qualified replacement for
the existing 5.15 production module. It is not in the production build/release
inputs or the native probe's artifact allowlist.

`provider.c` implements:

- One full-window exclusive resource claim with per-page RAM-PFN rejection.
- The unchanged two-slot ABI v3 and the separate Main-window ABI v1.
- Shared, read/write, non-executable WC mappings with fixed selectors/lengths.
- Prevention of later execute permission, VMA expansion, inheritance and dumps.
- Inspection of every newly mapped page through `follow_pfnmap_start/end`,
  rejecting PFN, write permission, memory-type, shareability or XN mismatch.
- Two root-only misc devices, with open refused until both registrations finish.
- Reverse-order rollback after partial registration and normal resource cleanup.
- Module ownership through file references, including VMA-held references after
  fd closure. Actual kernel lifetime/mprotect tests remain unperformed.

The internal registration routine requires platform qualification.
**`entry.c` always passes false**, so this module returns `EOPNOTSUPP` before
reserving memory or registering devices. There is no module parameter or ioctl
to bypass the gate. Removing it requires an implemented, reviewed platform
validator and satisfaction of the outstanding writer/attribute gates.

## Kernel APIs and build evidence

Linux 6.18 removed `no_llseek`; omitting the callback disables seeking through
the VFS open path. Do not replace it with `noop_llseek`, which allows seeking.

With `CONFIG_PER_VMA_LOCK=y`, `vm_flags_set/clear` introduce the GPL-only
`__vma_start_write` import. The provider instead uses the documented
`vm_flags_init()` API **only in its initial mmap callbacks**. In this pinned
kernel's `mm/vma.c::__mmap_new_vma`, the legacy file mmap callback runs before
`vma_iter_store_new`, so the VMA is not yet published. This is not valid for
editing an existing VMA and must not be reused in later permission callbacks.
No raw private-field mutation or export bypass is used.

The build script uses the same pinned source/config/compiler archives as the
successful probe. It reuses the SHA-256-verified `vmlinux.symvers` from the two
completed unmodified-kernel builds, whose export tables match exactly. It does
not fabricate symbols or use warning-only modpost. Inputs are mounted at
`/inputs`, this parent kernel directory at `/provider`, and a fresh output path
is passed to `bash /provider/scanout-618/build-in-container.sh` in the existing
Apple builder image (digest
`sha256:821fcb389464fb019f76bfa25eb8614ba92536072ab408066b3100a00dcfa1e7`).

Independent builds `build/window-provider-build-3` and `-4` produced identical
unstripped `.ko` files:
`9efe5df884aede9d3ca475773ac55364bc8ceb8cc7987937108bf30efb9098c6`.
Vermagic is `6.18.38-MiSTer SMP mod_unload ARMv7 p2v8`, no dependencies, and
all twelve imports passed modpost as ordinary exports. Build 4 retains source,
config, compiler, import and binary checksums. No provider has been device-loaded.

The host test includes the actual provider and entry-point C with mocked kernel
effects. It exercises the closed entry point, board/RAM rejection, claim failure,
both registration failures, open-before-publication rejection, rollback,
duplicate registration, mapping permissions/selectors, copy/remap failures,
both layout queries and idempotent cleanup. It does not prove kernel VMA lifetime
or target page-table attributes. The 58 focused host tests pass.

## Component licensing

The provider C sources and internal header are GPL-2.0-only, with
`MODULE_LICENSE("GPL")` and matching source-license metadata. The license text
is in `LICENSES/GPL-2.0-only.txt`. Nigel approved this component-specific grant;
the application, host tools and existing 5.15 module retain their licenses.
The three Main-window headers and slot UAPI header offer
`GPL-2.0-only OR GPL-3.0-or-later`, so the module and userspace can each select
the appropriate grant. Their ABI and implementations are unchanged. These
first-party files carry Nigel's copyright; no kernel implementation was copied.
The earlier build hashes above describe the pre-change artifact, not this source.

## Remaining qualification gates

The provider verifies its installed Linux PTE representation immediately after
`remap_pfn_range`, inside initial mmap with the mmap write lock held. Every
successful lookup is ended before returning or advancing; no result fields are
read after unlock. Failed lookups propagate their error and mismatches return
`EIO`. The pinned kernel's `mm/vma.c::__mmap_new_file_vma` calls `unmap_region`
when the callback fails, undoing the unsuccessful mapping. No memory contents
are read. The host fault tests cover first/last-page lookup failure, wrong PFN,
memory type, shareability, XN and writability, plus unrelated PTE-bit tolerance.

This is deliberately not a hardware-attribute attestation: ARM's Linux PTE
representation is not the raw hardware translation, and this check neither
reads PRRR/NMRR nor inspects pre-existing aliases or later permission changes.
It adds no diagnostic ioctl, arbitrary address access or activation bypass.

Independent builds 5 and 6 produce byte-identical modules and pass stock-kernel
modpost with both GPL-only lookup imports and matching `GPL` / `GPL-2.0-only`
module metadata. Their SHA-256 is
`d797b0532def95d6e89af4dc2b468a954703dd432049179e01c769cb5ac3f36c`;
vermagic remains `6.18.38-MiSTer SMP mod_unload ARMv7 p2v8`. The focused host
suite passes 58 tests. No provider has been loaded on the device.

The running config disables `CONFIG_ARM_PTDUMP_DEBUGFS`. The supported
`follow_pfnmap_start/end` helpers expose PFNs and mapping protections but are
GPL-only, as are `get_task_mm` and relevant device-enumeration helpers. The
approved GPLv2 provider can now use these supported exports.
Source-predicted attributes and resource reservation do not satisfy the promised
actual-target mapping verification.

No separately licensed diagnostic project or kernel fork is required. Inspection
still needs bounded operations, tests and separate approval before any device
load. PFN-mapping inspection does not automatically establish the stock driver's
alias attributes or prevent console/VT/mode writes.

Until the remaining platform checks are resolved, leave the
entry point closed. The stock config enables framebuffer console/VT, and the
driver's mode path clears the whole aperture. Main's admission gate alone cannot
exclude all those accesses. No unsafe activation or claim of completed migration
is justified by this provider build.
