# MiSTer MagiK UI tests

This package contains the attended, device-only UI test harness. It is kept
outside the repository's default pytest discovery path so CI can type-check and
lint it without launching a MiSTer or requiring the private Slint test wheel.

Install the optional `device-ui-tests` dependency group only on the operator
host that will run the suite. The Slint testing token must remain in the
operator environment and must never be committed or copied to the device.

The suite is attended and intentionally absent from CI discovery. Build the
isolated ARM binary with the typed build workflow, install it in the Dev slot,
then run selected journeys from the repository root with the device agent
configured:

```sh
export SLINT_TESTING_TOKEN="..."
export MISTER_UI_TEST_SSH_DESTINATION="root@192.0.2.10"
export MISTER_UI_TEST_COMMAND="/media/fat/mister-magik-dev/mister-magik-fb ui launcher 0"
scripts/agent build runtime-ui-tests
UV_INDEX="slint-private=https://testing.slint.dev/simple/" \
UV_INDEX_SLINT_PRIVATE_USERNAME=__token__ \
UV_INDEX_SLINT_PRIVATE_PASSWORD="$SLINT_TESTING_TOKEN" \
uv run --extra device-ui-tests python -m apps.mister.ui_tests.suite \
  startup-home system-hub arcade-navigation arcade-filters \
  settings-display screensaver-motion about-licenses effect-sandbox \
  profile-matrix \
  --fixture deterministic-arcade-v1 --attended
```

`slint-testing` launches the ARM process through its SSH reverse tunnel. The
test command receives only the bounded `MISTER_*`/`SLINT_*` controls; credentials
are filtered before a remote command is constructed. Virtual keyboard and
joystick devices are Linux-only and require uinput access where the Python
suite runs. SSH forwarding moves the Slint test protocol, not `/dev/uinput`, so
an operator running the suite on macOS cannot currently drive a remote MiSTer
with these virtual devices; use a Linux runner sharing the device kernel or add
a future typed device-side input relay. Use one operator session at a time so
the device display and input ownership remain unambiguous. The
`scripts/agent device launcher ui-test` command is the typed per-case handshake
used by the suite; it denies core launches, catalog writes, and reboots.
