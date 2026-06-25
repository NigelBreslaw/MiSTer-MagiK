# ControllerSetupInputSession / InputProfile Metrics

Parent: `e53a540cd6bbecc7cd934c7f70d898c46759ae5e`

Classification: correctness-only. No BEFORE/AFTER device benchmark was run
because this commit does not materially change joystick polling cadence,
framebuffer presentation, preview work, or launcher frame pacing. The Linux js
reader still drains the same 8-byte events and applies the same mapping logic;
the mapping is moved behind `InputProfile`.

Named correctness metric:

- Before: setup overlay consumed `PadPool::state()`, the merged state for all
  pads. A non-target pad pressing A could satisfy `SetupNav::handle_input` for
  the target pad.
- After: `ControllerSetupInputSession` routes setup to
  `PadPool::state_at(setup.target_pad_idx)` while launcher navigation continues
  to use the merged state.
- Evidence: `ui_runner::controller_setup_input_session::tests::setup_does_not_advance_from_non_target_pad_activity`
  passes and `setup_advances_from_target_pad_activity` passes.

Commands run:

- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features input_state`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features controller_setup_input_session`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features input::tests`
- `scripts/dev-rust test`
- `scripts/dev-rust check`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features`
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
- `git diff --check`

Optional check:

- `cargo clippy --manifest-path magik-gui/Cargo.toml --features ui --no-default-features --all-targets -- -D warnings`
  was attempted. It is not a repository-clean gate today: it fails on existing
  all-target warnings outside this item, including `permissions_set_readonly_false`,
  `needless_range_loop`, `too_many_arguments`, and other pre-existing binary/test
  lints. Touched-file clippy nits reported by that attempt were fixed before the
  final passing test/clippy runs above.
