# MiSTer MagiK UI tests

This package contains the attended, device-only UI test harness. It is kept
outside the repository's default pytest discovery path so CI can type-check and
lint it without launching a MiSTer or requiring the private Slint test wheel.

Install the optional `device-ui-tests` dependency group only on the operator
host that will run the suite. The Slint testing token must remain in the
operator environment and must never be committed or copied to the device.
