# Device operations and recovery

The Rust `mister` host binary is the operator interface for MiSTer access.
Maintained automation must use typed `DeviceRequest` operations; raw SSH/SCP
and generic remote-shell orchestration are not accepted interfaces.

Build the host tool through `scripts/agent check` or use the binary produced at
`mister/tools/host/target/debug/mister`. Common attended commands are:

```text
mister status
mister arming-status
mister mode status
mister mode dev
mister mode public
mister mode stock
mister scene launcher
mister --capture-buffer
```

Mode changes verify the selected platform manifest, preserve stock inittab,
mutate `MiSTer.ini` through the comment-preserving Rust editor, clear all
arming files, and use a bounded supervised reboot. Fixed non-launcher scenes
temporarily suspend the supervised launcher and always resume it.

## Boot-loop safety

Never arm reset faults through persistent `launcher.env`. Destructive recovery
requires an attended, volatile `/tmp` session token and a confirmed non-network
recovery path. Every cleanup must remove:

- `/media/fat/mister-magik/launcher.env`
- `/media/fat/mister-magik-dev/launcher.env`
- `/tmp/mister-magik/fs-fault-launcher.env`
- `/tmp/mister-magik/fs-fault-session`
- `/tmp/mister-magik/fs-fault.json`
- `/media/fat/mister-magik/rebuild-on-next-boot`
- `/media/fat/mister-magik-dev/rebuild-on-next-boot`

Use `mister arming-status` after recovery experiments. If the device repeatedly
reboots, stop deployment attempts. Power it down, mount the SD card on the Mac,
remove the listed files, and inspect
`/media/fat/mister-magik/bootlogs/main-reboot.log`.

## Agent workflows

Agents do not call the operator tool. Runtime and platform changes are committed
first and then use `scripts/agent deliver`. Performance work uses
`scripts/agent benchmark`; diagnosis uses `scripts/agent diagnose`. The attended
release gate is `scripts/agent release qualify`.

Device mutation is serialized, bounded, snapshotted, and compensated. A device
timeout, refusal, route, or authentication failure ends after the first typed
attempt and is reported as unavailable. Host-only work never contacts MiSTer.

## Display evidence

Production rendering is RGB565. `/dev/fb0` contents alone do not prove HDMI
visibility. Use Analytics streaming for continuous inspection and
`mister --capture-buffer` for a still, then pair it with attended HDMI evidence
when making scan-out claims.
