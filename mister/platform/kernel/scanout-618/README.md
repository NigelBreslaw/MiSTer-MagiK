# Linux 6.18 fixed-window provider

This directory contains the short-term out-of-tree provider for the self-built
MiSTer Linux `6.18.38-MiSTer` development platform. It is not part of MagiK's
release inputs.

The ordinary object is intentionally activation-disabled and returns
`EOPNOTSUPP` before reserving memory or creating a device. The explicitly
requested development build produces `mister_magik_scanout_slots.ko` with
`mister_magik_development_trial=stock-6.18-latch-reuse-v3`.

## Fixed interfaces

The module publishes two root-only misc devices after all preflight checks pass:

- `/dev/mister-magik-main-window`: one exact mapping of the complete fixed
  physical window.
- `/dev/mister-magik-scanout-slots`: the unchanged two-slot ABI v3.

It claims the complete window exclusively, rejects System RAM PFNs, and accepts
only the fixed selectors and lengths. Mappings must be shared, writable,
non-executable and write-combined. Every installed page is checked with
`follow_pfnmap_start/end`; PFN and writability mismatches fail with `EIO`.

The trial-only read-only diagnostic ioctl returns one fixed 64-byte, per-open
record. ABI v2 reports the first mapping failure and records which VMA,
write-combine, execute-never, writable, shared, supplied-protection, PFN and
writability checks completed. It cannot select an address, modify a mapping or
bypass validation.

The provider keeps module ownership through file and VMA references, registers
both devices atomically, and unwinds registrations and the resource claim in
reverse order.

## Exact development kernel

`build-in-container.sh` accepts only the inputs used for the working paired
kernel/module build:

- Kernel source: MiSTer `MiSTer-v6.18` revision
  `6a581bac47c32dfd2525f9874fd263cf08058610`.
- Kernel archive SHA-256:
  `0702694110b54441b0a8323be43538e1d5b394645c4970616867b56e55496672`.
- Final `.config` SHA-256:
  `584c7fdb7884616363b38c0514266a5fc40083ae327d9a71e72deb6f3101cdab`.
- Final `Module.symvers` SHA-256:
  `f58b220d8cdcb925afdd4ba4a4c0a04c02154a8f2fc658cc1fa885b89f79952f`.
- Arm GNU toolchain archive SHA-256:
  `d169f9196e3a6c4248ee79ca85987ebce0e4ea9174c1f8d51af9b28fecf22da1`.
- Vermagic: `6.18.38-MiSTer SMP mod_unload ARMv7 p2v8`.

Mount those files at `/inputs` as `kernel.tar`, `kernel.config`,
`vmlinux.symvers` and `toolchain.tar.xz`; mount this `kernel` directory at
`/provider`. Build the usable development object with:

```sh
bash /provider/scanout-618/build-in-container.sh \
    --development-trial /outputs/trial
```

Omitting `--development-trial` builds the activation-disabled review object.
Each output directory contains the object, verified inputs, compiler and module
metadata, imports, source checksums and `SHA256SUMS`. Generated objects are not
committed.

## Verification

The focused host tests include the actual C implementation with mocked kernel
effects. They cover the disabled ordinary entry point, exact trial gate,
platform/RAM/resource rejection, partial registration rollback, per-open
diagnostics, fixed mappings and selectors, all mapping failure categories, and
normal teardown.

Both objects build without modpost warnings against the exact self-built kernel.
The PR-source trial object has SHA-256
`d69534405c3869a17ea88ef57a291cd26877599916b695453c7bc8275fa5650f`.
After removing debug information and the build-ID note, it is byte-identical to
the module used in the successful device run; both reduce to SHA-256
`33633b174d0ef331ec3e339910c8ae6eda463984d4c1f5ac31ce92f7d4acc2ee`.
The device run successfully created both fixed mappings on the paired kernel and
completed MagiK presentations without module, mapping or ownership failures.

## Scope and licensing

The provider C files are `GPL-2.0-only` and declare `MODULE_LICENSE("GPL")`.
Shared UAPI headers offer `GPL-2.0-only OR GPL-3.0-or-later`, allowing the module
and existing GPLv3 userspace to choose their compatible grant independently.

This development provider is not a generic physical-memory mapper, allocator,
presentation engine or writer lease. It does not prove FPGA DDR-read quiescence
or promote the Linux 6.18 path to production. Production activation remains a
separate qualification and packaging decision.
