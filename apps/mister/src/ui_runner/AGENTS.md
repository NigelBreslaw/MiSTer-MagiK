# AGENTS.md - launcher runtime

Root and `apps/mister/AGENTS.md` apply.
File authority is documented in `docs/agents/file-authority.md`.

## Ownership

This directory owns launcher startup, scheduling, lifecycle, catalog/media
workers, presentation, composition, Slint bridging, and benchmark scenarios.
Start with `launcher_loop.rs`, then follow the named session/lifecycle module
for the behavior being changed. Use `docs/architecture.md` state charts as the
ordering source of truth.

## Rules

- Preserve lifecycle, composition, and display ownership invariants.
- Do not perform cache creation, network work, or blocking persistence on the
  UI hot path.
- Keep benchmark-only policy isolated from production defaults.
- A full Slint present invalidates direct Arcade layers; repaint them in the
  same frame when Arcade remains active.
- Keep scanout cadence and latch protocol health as separate gates. A zero
  latch drop count does not prove that every physical refresh received a new
  frame. Only a validated protocol-v5 `repeated_vblank_count` delta may populate
  `dropped_frames`; completion timing remains diagnostic. Every authoritative
  animation window requires exactly zero FPGA repeats.
- Tests should exercise event/state sequences, not only helper predicates.

## Assurance

Use `$magik-rust-lsp` while editing Rust and the Slint MCP for UI behavior.
Staged formatting and policy run at pre-commit; tests, Clippy, compiled UI, and
ARM assurance run at pre-push and in CI. Device validation is required only
when behavior depends on scan-out, timing, input hardware, or Main handoff.
