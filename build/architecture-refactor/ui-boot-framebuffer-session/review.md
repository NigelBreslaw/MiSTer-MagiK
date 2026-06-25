# UiBootFramebufferSession Review

Reviewer: Hooke (`019efc3e-c01f-7341-9aa2-3417572eb257`)

Initial finding:

- P2: diagnostics builds gained new warning-deny failures because the new
  `FpgaFramebufferRoute` callers left `Fpga::fb_enable` and
  `Fpga::fb_enable_direct` unused, and `with_offsets` was dead outside
  experiments.

Fix:

- Removed the now-redundant `Fpga::fb_enable` and `Fpga::fb_enable_direct`
  wrappers, leaving `fb_enable_format` as the canonical low-level FPGA route
  adapter.
- Gated `FpgaFramebufferRoute::with_offsets` and its unit test behind
  `mister_experiments`, matching its effect-benchmark use.

Final findings: none.

Final verdict:

- Clean for the item-8 scope.
- Framebuffer mode guard lifetime is preserved by destructuring the boot session
  into an owned `_fb_mode_guard` binding in `run_ui`.
- Boot analytics/log names are preserved.
- Full UI routing, early-black routing, diagnostics routing, and launcher
  recovery/reassertion keep route parameter parity.

Reviewer verification:

- `cargo check --features ui --no-default-features`: pass.
- `cargo check --features ui,experiments --no-default-features`: pass.
- `cargo test --features ui --no-default-features ui_runner::ui_boot`: pass.
- `cargo test --features ui,experiments --no-default-features ui_runner::ui_boot`: pass.
- `cargo check --features ui,diagnostics --no-default-features` still fails only
  on pre-existing/out-of-scope dead-code warnings; the previous `fpga.rs` and
  `with_offsets` failures are gone.
