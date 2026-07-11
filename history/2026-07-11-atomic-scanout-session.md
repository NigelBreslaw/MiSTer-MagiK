# Atomic scanout userspace session — 2026-07-11

Status: implemented behind the existing opt-in; device qualification pending the
replacement RBF build.

## Confirmed cause

The zero-copy proof rendered Slint into cacheable plugin pages, but ownership
transfer and route posting were separate operations. It also composed preview
and Arcade direct layers into the legacy write-combined hidden mapping, and the
desktop stream could read the framebuffer after a post. That was incompatible
with hard PTE revocation: every writer must finish before one atomic post, and
no CPU read or write may touch the selected slot until the FPGA completion fence
releases it.

## Before / after

- Before: two control operations per frame (`SYNC_DEVICE`, then UIO `0x57`),
  zero verified mailbox sessions, zero direct-layer writers targeting the
  cacheable scanout slot, and one possible post-fence stream read.
- After: one `SYNC_RANGES_AND_POST` ioctl transfers ownership and publishes the
  route; one epoch-verified mailbox bootstrap per session; Slint, preview, and
  Arcade writers share the selected cacheable slot and contribute damage; the
  stream snapshot is copied before ownership transfer; zero post-fence reads.
- A normal early next frame waits up to 25 ms for the kernel/FPGA fence instead
  of touching a revoked mapping or incorrectly disabling the path.
- Production work-p99 remains Home 6,888 us, Arcade 3,736 us, preview 2,469 us
  until this exact path is built, deployed, and benchmarked.

## Tests

- `cargo check --manifest-path magik-gui/Cargo.toml --features ui --no-default-features`
- `cargo test --manifest-path magik-gui/Cargo.toml --lib --features ui --no-default-features`
  — 305 passed before the added scanout-target regression, then 105 focused
  framebuffer tests passed.
- `cargo test --manifest-path magik-gui/Cargo.toml --bin mister-magik-fb --features ui --no-default-features`
  — 517 passed.
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
- `git diff --check`

## Evidence artifacts

- `magik-gui/src/framebuffer/plugin_probe.rs`
- `magik-gui/src/framebuffer/target.rs`
- `magik-gui/src/ui_runner/launcher_present/latch.rs`
- `magik-gui/src/ui_runner/launcher_latch_state.rs`
- `history/2026-07-11-production-zero-copy-baseline.md`
- `history/2026-07-11-true-zero-copy-slint-proof.md`
