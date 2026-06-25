# ControllerSetupInputSession / InputProfile Review

Reviewer: Godel (`019efc33-1ec3-72c3-b656-022ee7b47475`)

Findings: none.

Notes:

- Active setup uses the target pad via
  `controller_setup_input_session.rs:21`.
- `launcher_loop.rs` passes the isolated setup state to
  `setup.handle_input`, while normal launcher navigation continues to use the
  merged launcher state after setup is inactive.
- Raw joystick mapping parity is preserved by `InputProfile::apply_js_event`;
  the generic and `dpad_axes_4_5` button/axis behavior matches the old
  `PadStateEventExt` implementation.

Verdict: approved for a correctness-only commit. The mapping move is mechanical,
covered by focused parity tests, and supports the setup-focus correction without
scope creep.
