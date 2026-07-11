# Atomic scanout observability — 2026-07-11

## Confirmed cause

The Agent could report processes and proxy framebuffer frames, but it could not
distinguish an atomic scanout session from the legacy latch fallback. Desktop
captures also lacked an explicit statement that their source snapshot was taken
before the kernel revoked CPU access to a posted slot.

## Before / after

- Before: zero scanout status objects, zero renderer state labels, and zero
  ownership-safety fields in the stream handshake.
- After: one Agent `scanout` status object combines actual module/device facts,
  Main readiness, and Slint mode/state; the renderer publishes five explicit
  states; the stream handshake publishes source and ownership safety.
- This is observability-only on the hot path. Production work-p99 remains Home
  6,888 us, Arcade 3,736 us, preview 2,469 us pending device AFTER runs.

## Tests

- `cargo test --manifest-path tools/magik-agent/Cargo.toml`
- `cargo clippy --manifest-path tools/magik-agent/Cargo.toml --all-targets -- -D warnings`
- `cargo test --manifest-path magik-gui/Cargo.toml --lib --features ui --no-default-features runtime_status`
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
- `git diff --check`

## Evidence artifacts

- `tools/magik-agent/src/main.rs`
- `magik-gui/src/runtime_status.rs`
- `docs/magik-agent.md`
- `history/2026-07-11-atomic-scanout-session.md`
