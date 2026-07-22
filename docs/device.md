# Device operations and recovery

The Rust `mister` host binary is the operator interface for MiSTer access.
Maintained automation must use typed `DeviceRequest` operations; raw SSH/SCP
and generic remote-shell orchestration are not accepted interfaces.

Public installation and removal are handled by the dedicated Rust
`mister-magik-manager`; see [installer.md](installer.md). The Scripts-menu shell
file is only a fail-closed, hash-verifying bootstrap.

Build or refresh the host tool through `scripts/agent check`, then use the binary
produced at `mister/tools/host/target/debug/mister`. Protocol and host-tool
changes make the harness rebuild that runnable binary before device access.
Common attended commands are:

```text
mister status
mister arming-status
mister mode status
mister mode dev
mister mode public
mister mode stock
mister scene launcher
mister --capture-buffer
mister display-mode hdmi-1280x720p60 --attended
mister display-mode hdmi-1920x1080p60 --attended --keep
```

`display-mode` applies one provisional Main-owned transaction, verifies the
replacement launcher and framebuffer geometry, and rolls back unless `--keep`
is supplied. It never bypasses the confirmation transaction.

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

The attended release gate qualifies whichever MagiK layout is currently active.
Its display phase reboots through the fixed boundary matrix: presets 10, 13,
14, 8, and 0, followed by custom `1920,1200,60`. Every case requires the ABI
v3 1366x768 latch contract, its expected launcher framebuffer, zero FPGA drops,
and a captured framebuffer. The final restoration puts back the original INI
and performs a supervised reboot even when a qualification step fails.

## Display evidence

Production rendering is RGB565. `/dev/fb0` contents alone do not prove HDMI
visibility. Use Analytics streaming for continuous inspection and
`mister --capture-buffer` for a still, then pair it with attended HDMI evidence
when making scan-out claims.

Framebuffer capture v2 reads the FPGA-latched hidden RGB565 slot while MagiK
scan-out slots are available. A latch, status, or slot-mapping failure is
reported directly and never replaced with a successful `/dev/fb0` capture.
`/dev/fb0` remains the explicit legacy source when scan-out slots are absent.
Delivery smoke requires authoritative, nonblank hidden-slot content; manual
captures may still represent a legitimately blank authoritative frame.

Interactive captures are saved to the Desktop. When stdout is redirected, the
capture is saved under the OS temporary directory at
`mister-magik/captures/`, and the command prints a Markdown link to the PNG so
an image-capable agent can inspect it without receiving base64 text.

`mister display-matrix --attended --out DIRECTORY` performs the bounded runtime display
matrix without rebooting Linux. Main applies each supported resolution as a
provisional transaction, the launcher restarts, and the authenticated MagiK
agent returns one PNG per mode. The command writes deterministic PNG names and
`manifest.json`, cancels every provisional mode after capture, and verifies the
original working mode is restored between cases. It requires an interactive
`31KHZ` acknowledgement because the matrix includes 480p/576p CRT modes, then
checks launcher PID replacement, output/framebuffer geometry, advancing frame
counters, RGB565 stride, nonblank content, and unique capture hashes. The v2
manifest records partial failures before cleanup. Framebuffer evidence still
requires attended sink observation for HDMI or CRT visibility claims.

The launcher keeps output geometry one-to-one through 1280x720 and uses half
width and height when the output is at least 1366 pixels wide or 900 pixels
high. Custom modes use the same rule. The qualified ABI v3 hidden slots support
up to 1366x768 with a 2736-byte RGB565 stride and 2,101,248 bytes per slot.
