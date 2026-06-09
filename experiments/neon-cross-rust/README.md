# NEON Cross Rust Experiment

Small standalone proof that Rust can call NEON code in a MiSTer-compatible
`armv7-unknown-linux-gnueabihf` binary without compiling the main frontend.

This intentionally avoids `core::arch::arm` because stable Rust currently
rejects the direct ARM NEON intrinsics used in earlier experiments. Instead,
`build.rs` compiles one C file with `<arm_neon.h>` using the cross container's
ARM GCC, archives it into a static library, and links it into a Rust binary.
The C helper is compiled with `-mcpu=cortex-a9 -mfpu=neon -mfloat-abi=hard`,
matching the MiSTer's ARM core.

```bash
cd experiments/neon-cross-rust
./build.sh
```

Expected output includes:

```text
Compiling C NEON probe with arm-linux-gnueabihf-gcc
Finished `dev` profile ...
```

The resulting binary is:

```text
target/armv7-unknown-linux-gnueabihf/debug/neon-cross-rust
```

To smoke-test it on the MiSTer from the repo root:

```bash
scripts/mister put experiments/neon-cross-rust/target/armv7-unknown-linux-gnueabihf/debug/neon-cross-rust /tmp/neon-cross-rust
scripts/mister run "chmod +x /tmp/neon-cross-rust; /tmp/neon-cross-rust"
```
