# Device operations and recovery

The Rust `mister` host binary is the operator interface for MiSTer access.
Maintained automation must use typed `DeviceRequest` operations; raw SSH/SCP
and generic remote-shell orchestration are not accepted interfaces.

Public installation and removal are handled by the dedicated Rust
`mister-magik-manager`; see [installer.md](installer.md). The Scripts-menu shell
file is only a fail-closed, hash-verifying bootstrap.

Push the committed host-tool change through pre-push assurance, then use the
binary produced at `mister/tools/host/target/debug/mister`. Protocol and
host-tool changes make the assurance harness rebuild that runnable binary before
device access.
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

Never arm reset faults through persistent `launcher.env`. Reset-fault testing
and `direct-reset-no-sync` require an attended, volatile `/tmp` session token
and a confirmed non-network recovery path. Every cleanup must remove:

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

An unattended one-shot recovery reboot is permitted and is not reset-fault
testing. `scripts/agent diagnose` may remove the listed arming files, verify
that the device is not marked reboot-unstable, issue one raw Linux reboot over
SSH, and wait for authenticated agent, SSH, and launcher health. It never
automatically replays the reboot request.

## Agent workflows

Agents use closed typed workflows rather than the operator CLI. Runtime and
platform changes are committed first and then use `scripts/agent deliver`.
Committed Main-fork experiments use `scripts/agent deliver local-main`, which
is Dev-only and replaces only the verified Main/manifest pair from an exact
clean sibling checkout. The first deployment of reload support may use one
Linux reboot; later experiments activate by supervised Main replacement.
Performance work uses `scripts/agent benchmark`; diagnosis and one-shot reboot
recovery use `scripts/agent diagnose`. The attended release gate is
`scripts/agent release qualify`.

Device mutation is serialized, bounded, snapshotted, and compensated.
Explicitly read-only requests receive one bounded retry after a transient
timeout, refusal, or route failure. Mutating requests are never blindly
replayed; their owning state machine must reconcile or compensate first.
Authentication and access failures require changed external state. The device
is reported unavailable only after the applicable bounded recovery fails.
Host-only work never contacts MiSTer.

The runtime executable and its platform manifest are inseparable deployment
state. No operator command, benchmark request, or device-agent endpoint may
replace `mister-magik-fb` alone. Runtime changes use the coherent Dev bundle
transaction, activate the regenerated manifest last, and retain rollback until
Main has passed preflight and the launcher has proved ready on the real latch
path. A Main suspend/resume acknowledgement confirms command acceptance only;
it is never deployment health evidence. The host tool intentionally exposes no
generic remote shell, file upload, directory upload, or binary deployment
subcommand; fixed typed operations own all device mutation.

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

On macOS, `scripts/agent capture usb-video [--output PATH]` captures the first
nonblank 1920x1080 frame from the fixed `USB Video` input. The native
AVFoundation path writes JPEG, refuses to overwrite an explicit output, and
otherwise saves a unique file under the same temporary capture directory before
printing a Markdown link. `--seconds N` selects a bounded 1–60 second native
AVFoundation movie capture at the same 1920x1080 format and the `USB Video`
device's 30 fps ceiling; its output is a `.mov` file and is intended for
attended motion diagnosis. Both are sink evidence, not a replacement for the
authoritative framebuffer capture.

Qualified-black platform candidates also require the attended evidence set in
`docs/bootstrap-black-qualification.md`. It covers cold reboot, active launcher
restart without RBF reload, game return, and injected preflight failure. Each
movie is retained beside Main events/status and latch status; none of these
physical checks may be replaced by `/dev/fb0` inspection.

`mister display-matrix --attended --out DIRECTORY [--usb-video]
[--screensaver-wait SECONDS]` performs the bounded runtime display
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

With `--usb-video`, each case also captures the fixed `USB Video` input after
the authoritative framebuffer capture through the native agent capture command,
and records its path, size, and hash in the manifest. CRT/VGA acceptance
requires routing Morph 4K Port B before those cases and restoring HDMI afterward.
Morph credentials are runtime-only operator input and must never be passed as
command arguments or written to artifacts.
`--screensaver-wait` adds a second authoritative framebuffer capture after the
bounded idle interval and, when combined with `--usb-video`, a second sink
capture. The case fails if screensaver content is not distinct from the launcher.

The launcher keeps output geometry one-to-one through 1280x720 and uses half
width and height when the output is at least 1366 pixels wide or 900 pixels
high. Custom modes use the same rule. The qualified ABI v3 hidden slots support
up to 1366x768 with a 2736-byte RGB565 stride and 2,101,248 bytes per slot.
