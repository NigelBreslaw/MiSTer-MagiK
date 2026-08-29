# MiSTer MagiK UI tests

This package contains the attended, device-only UI test harness. It is kept
outside the repository's default pytest discovery path so CI can type-check and
lint it without launching a MiSTer or requiring the private Slint test wheel.

Install the optional `device-ui-tests` dependency group only on the operator
host that will run the suite. The Slint testing token must remain in the
operator environment and must never be committed or copied to the device.

Initialize the private asset gitlink in every new worktree before building. The
status must show the expected revision with no leading `-`, `+`, or `U` marker:

```sh
git submodule update --init private/magik-assets
git submodule status -- private/magik-assets
```

The suite is attended and intentionally absent from CI discovery. Build the
isolated ARM binary with the typed build workflow, install it in the Dev slot,
then run the smoke journey before the complete suite. On macOS, this typed
workflow uses the Apple `container` ARM backend; `cross` is only an explicit
alternate comparison backend. The first typed device operation upgrades an
older installed agent transactionally; confirm its status reports agent
version 34 before relying on the renamed `menu-confirmations` case.

```sh
export SLINT_TESTING_TOKEN="..."
export MISTER_IP="192.0.2.10"
export MISTER_DEVICE_ID="mister-living-room"
scripts/agent build runtime-ui-tests
UV_INDEX="slint-private=https://testing.slint.dev/simple/" \
UV_INDEX_SLINT_PRIVATE_USERNAME=__token__ \
UV_INDEX_SLINT_PRIVATE_PASSWORD="$SLINT_TESTING_TOKEN" \
uv run --extra device-ui-tests python -m apps.mister.ui_tests.suite \
  smoke --fixture deterministic-arcade-v1 --attended
```

The complete suite is:

```sh
UV_INDEX="slint-private=https://testing.slint.dev/simple/" \
UV_INDEX_SLINT_PRIVATE_USERNAME=__token__ \
UV_INDEX_SLINT_PRIVATE_PASSWORD="$SLINT_TESTING_TOKEN" \
uv run --extra device-ui-tests python -m apps.mister.ui_tests.suite \
  startup-home system-hub arcade-navigation arcade-filters \
  settings-display screensaver-motion about-licenses \
  menu-confirmations \
  profile-matrix \
  --fixture deterministic-arcade-v1 --attended
```

Expand device qualification in this order, stopping at the first failure:

1. Run `smoke`.
2. Run `system-hub`.
3. Run `arcade-navigation`, `arcade-filters`, `settings-display`,
   `screensaver-motion`, and `about-licenses` independently.
4. Run `menu-confirmations`.
5. Run `profile-matrix` (12 display/orientation/feature sessions: HDMI
   1920×1080 and CRT 240p, each in normal and monitor-left orientation across
   three views).
6. Run the complete command once.
7. Run the complete command a second time immediately afterward.

Only the two consecutive complete runs qualify the suite. A diagnostic rerun
of one failed case helps isolate a fault but does not count as qualification;
the ladder restarts from `smoke` after a fix. Suite subprocesses fail if pytest
reports a skip, so a green run contains no silently omitted test.

The controller screen is a future feature and is therefore excluded from this
qualification ladder. Its explicit test target remains available for a later
feature milestone.

`slint-testing` stays on the operator host. Its local test socket is bridged by
the typed MagiK agent connection, while logical keyboard and joystick actions
are sent through the launcher's authenticated automation queue. No touchscreen,
SSH forwarding, or `/dev/uinput` access is required. The agent stages the exact
verified ARM test runtime in volatile `/tmp` storage, suspends the normal
launcher for the session, and resumes it on success, timeout, disconnect, or
failure. Use one operator session at a time so display and input ownership
remain unambiguous. Dangerous test effects are intercepted by the test runtime
and never reach production settings, catalog files, reboot, or core-launch
paths.

`SLINT_TESTING_TOKEN` is used only by `uv` on the operator host to install the
private `slint-testing` wheel. It is removed from the bridge environment and is
never copied to the MiSTer. The MagiK agent credential is separate and remains
inside the typed host client; it is not included in test payloads or runtime
environment variables.

The suite is attended and intentionally absent from CI discovery. CI still
formats, lints, and type-checks every Python harness file with Ruff and `ty`.
