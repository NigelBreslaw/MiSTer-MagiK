# NEON Cross Rust Experiment

Small standalone proof that Rust can compile NEON code in a MiSTer-compatible
`armv7-unknown-linux-gnueabihf` binary without compiling the main frontend.

This intentionally uses Rust's `core::arch::arm` intrinsics directly. It does
not compile or link any C helper. ARM NEON intrinsics are still behind
`#![feature(stdarch_arm_neon_intrinsics)]`, so this experiment uses the repo's
nightly toolchain instead of stable Rust.

`build.sh` sets `-C target-cpu=cortex-a9 -C target-feature=+neon`, matching the
MiSTer's ARM core.

```bash
cd experiments/neon-cross-rust
./build.sh
```

Expected output includes:

```text
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
