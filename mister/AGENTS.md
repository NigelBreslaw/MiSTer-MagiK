# MiSTer integration

`platform/contracts/` owns checked ABI representations; `platform/runtime/`
adapts hardware to portable interfaces. Keep file descriptors, ioctls, physical
addresses, and Main commands out of portable code. Source moves must preserve
installed `/media/fat/mister-magik/**` paths and platform identity.
