# Device operations and recovery

`scripts/agent device` is the operator interface for MiSTer access. The
MiSTer-side `mister-magik-agent` remains a separate service. Raw SSH/SCP and
generic remote-shell orchestration are not accepted interfaces.

Public installation and removal are handled by the dedicated Rust
`mister-magik-manager`; see [installer.md](installer.md). The Scripts-menu shell
file is only a fail-closed, hash-verifying bootstrap.

Push committed host-tool changes through the bootstrap-free Python pre-push
assurance. Device operations themselves continue through the repository's sole
typed `scripts/agent` entrypoint, which builds its Rust client when required.
Common attended commands are:

```text
scripts/agent device status
scripts/agent device arming-status
scripts/agent device mode status
scripts/agent device mode set dev --attended
scripts/agent device mode set public --attended
scripts/agent device mode set stock --attended
scripts/agent device scene launcher --attended
scripts/agent device capture framebuffer
scripts/agent device display route-status
scripts/agent device display set hdmi1280x720p60 --attended
scripts/agent device display set hdmi1920x1080p60 --attended --keep
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

Use `scripts/agent device arming-status` after recovery experiments. If the device repeatedly
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
Already-published game databases use the isolated
`scripts/agent deliver game-databases --game-databases-release-dir PATH` target.
It validates the local release and invokes only the rollback-capable database
transaction; it never resolves or changes a platform, runtime, Main, kernel,
or FPGA artifact.
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

Delivery reconciles the active FPGA during every non-platform delivery. A current
diagnostic identity preserves a runtime-only delivery; stale, incomplete, or
repairable FPGA evidence promotes that same invocation to the rollback-capable
platform transaction. Delivery never requires a second invocation to finish FPGA
reconciliation. After platform reboot, smoke waits for bounded readiness, performs at
most one Main-owned reload for a definite stale identity, and reports the failed
identity/readiness checks before rollback.

The runtime executable and its platform manifest are inseparable deployment
state. No operator command, benchmark request, or device-agent endpoint may
replace `mister-magik-fb` alone. Runtime changes use the coherent Dev bundle
transaction, update the v3 manifest's GUI and MagiK identity fields, activate
the regenerated manifest last, and retain rollback until Main has passed
preflight and the launcher has proved ready on the real latch path. Runtime
delivery uses Main suspend/resume and does not reboot Linux; platform changes
retain the complete manifest-bound transaction and its supervised reboot. A
Main suspend/resume acknowledgement confirms command acceptance only; it is
never deployment health evidence. The host tool intentionally exposes no
generic remote shell, file upload, directory upload, or binary deployment
subcommand; fixed typed operations own all device mutation.

The large Runtime executable is transferred byte-for-byte over the existing
authenticated agent connection initiated by the host. The v1 receiver accepts
only the declared byte count and lowercase SHA-256, requires the active Dev
deploy lock, and can stage only the canonical `mister-magik-fb.upload`; it
cannot activate a binary or alter its manifest. The host shuts down its write
side after the payload so the agent can reject truncation and surplus data,
then the normal transaction independently verifies SHA-256 before swapping the
binary and regenerated manifest together. An ambiguous transfer result is
reconciled by exact staged size and hash. A mismatch is removed and confirmed
absent before one SFTP fallback. Artwork and manifests remain on SFTP. The host
opens no inbound transfer listener, avoiding the macOS incoming-connection
permission path.

The attended release gate qualifies whichever MagiK layout is currently active.
Its display phase reboots through the fixed boundary matrix: presets 10, 13,
14, 8, and 0, followed by custom `1920,1200,60`. Every case requires the ABI
v3 1366x768 latch contract, its expected launcher framebuffer, zero FPGA drops,
and a captured framebuffer. Zero FPGA drops qualifies protocol integrity only;
motion qualification separately requires physical-refresh cadence evidence.
The final restoration puts back the original INI
and performs a supervised reboot even when a qualification step fails.

## Display evidence

Production rendering is RGB565. `/dev/fb0` contents alone do not prove HDMI
visibility. Use Analytics streaming for continuous inspection and
`scripts/agent device capture framebuffer` for a still, then pair it with attended HDMI evidence
when making scan-out claims.

`scripts/agent device display route-status` reads the live FPGA video height
and framebuffer-route parameters through Main's UIO ABI. It requires the
launcher to remain active with MagiK owning the FPGA, and does not acquire
display ownership, change the route, restart Main, or reload the RBF. Use it to
retain the failing state while distinguishing a disabled framebuffer route from
a fault farther downstream in the FPGA/HDMI path.

Framebuffer capture v2 reads the FPGA-latched hidden RGB565 slot while MagiK
scan-out slots are available. A latch, status, or slot-mapping failure is
reported directly and never replaced with a successful `/dev/fb0` capture.
`/dev/fb0` remains the explicit legacy source when scan-out slots are absent.
Delivery smoke requires authoritative, nonblank hidden-slot content; manual
captures may still represent a legitimately blank authoritative frame.

Interactive captures are saved to the Desktop. When stdout is redirected, the
capture bundle is saved under the OS temporary directory at
`mister-magik/captures/`, and the command prints labeled Markdown links so an
image-capable agent can inspect it without receiving base64 text. With
`--output STEM`, the same bundle is written beside the requested stem; an
optional `.png` suffix is stripped.

For authoritative 15 kHz captures (`640×240` or `640×288`), the bundle has
three files:

- `STEM-raw.png` — the byte-preserved authoritative scanout capture.
- `STEM-raw-letterbox-4x3.png` — the raw square-pixel raster centered on a
  black `640×480` canvas, with no source pixels scaled.
- `STEM-display-4x3.png` — a `640×480` nearest-scanline preview representing
  the physical 4:3 display aspect.

The latter two are inspection views, not new scanout evidence. Other routes or
non-authoritative sources produce only the raw artifact. The deterministic
Arcade fixture is available through
`scripts/agent device launcher capture-first-arcade --attended --output STEM`.

Use
`scripts/agent device launcher verify-neogeo-sdram --attended --output DIRECTORY`
after installing a Main candidate. It runs the installed Metal Slug 3
structured plan, a second high-memory NeoGeo plan, a low-memory control, and a
real `.mgl` entry. Each core is observed through USB video while active and is
returned through Main's typed launcher recovery path. Passing requires the
operator to confirm correct title/attract graphics with no memory warning and
the Main event stream to prove 128 MiB SDRAM was configured before handoff.

For the first CRT font A/B review, use the attended row-phase harness:
`scripts/agent device launcher capture-crt-font-ab --attended --pair row-phase --output STEM`.
It switches to 240p for the pair, restores the prior route afterward, and
writes labeled A/B bundles plus a side-by-side true-4:3 comparison.

For the font-only coverage experiment, restart the launcher with
`scripts/agent device launcher restart --attended --crt-font-experiment coverage-max`,
open the Arcade list manually, and capture only after the operator confirms the
fixture is visible. The one-shot experiment does not refresh or replace the
catalog.

For the non-merging follow-up, use
`scripts/agent device launcher restart --attended --crt-font-experiment dominant-row`.
It selects one complete glyph row for each absolute two-row group based on total
coverage, preferring the production odd row on ties, and leaves the catalog
untouched. Again, capture only after the operator confirms the Arcade list is
visible.

For the Xerxes typeface comparison, use
`scripts/agent device launcher restart --attended --crt-font-experiment xerxes`.
This selects the existing exact-size Xerxes 10 bitmap resource only for CRT240
Arcade titles, applies no glyph-row reconstruction, and does not touch the
catalog. Capture only after the operator confirms the Arcade list is visible.

For the pixel-perfect Xerxes comparison, use
`scripts/agent device launcher restart --attended --crt-font-experiment xerxes-perfect`.
It selects the generated 32px Xerxes resource only for CRT240 Arcade titles
and game rows. Each design cell becomes a 2×2 composition block and survives
the unchanged centered 480→240 conversion exactly; the catalog is untouched.

For the pixel-perfect Yesterday comparison, use
`scripts/agent device launcher restart --attended --crt-font-experiment yesterday-perfect`.
It selects the generated 32px Yesterday resource only for CRT240 Arcade titles
and game rows, uses unchanged centered scanout with exact 2×2 composition
cells, and leaves the catalog untouched.

For the pixel-grid Bacteria 12 comparison, use
`scripts/agent device launcher restart --attended --crt-font-experiment bacteria`.
It selects the generated 32px Bacteria resource only for CRT240 Arcade titles
and game rows. Each 64-unit design cell becomes a 2×2 composition block, so the
normal centered 480→240 conversion retains the intended bitmap exactly. It
does not use glyph-row reconstruction or touch the catalog.

For the direct half-size comparison, use
`scripts/agent device launcher restart --attended --crt-font-experiment bacteria-half`.
It uses the native 16px Bacteria resource with unchanged centered scanout and
no row reconstruction. Its 12-row capitals reduce to roughly six CRT
scanlines; the catalog remains untouched.

CRT UI typography uses Jersey 25 for major headings, Spleen bitmap resources
for settings and compact status text, and Nocive 15 for footer hints. Press
Start 2P is not linked into the runtime UI; only its pre-rasterized `MiSTer`
and `MagiK` particle targets remain. CRT240 Arcade game titles default to the
pixel-perfect 32px Yesterday 10 resource, while Arcade metadata uses native
Spleen. Typography selection does not refresh or modify the catalog.

On macOS, `scripts/agent capture usb-video [--output PATH]` is the sole
supported host capture interface for the fixed `USB Video` input. With no
duration it captures the first nonblank 1920x1080 frame and writes JPEG.
`--seconds N` records a bounded 1–60 second 1920x1080 QuickTime movie at the
device's native 30 fps ceiling. Both operations use the same native
AVFoundation device discovery and format configuration, refuse to overwrite an
explicit output, and otherwise allocate a unique temporary path.

Movie success is fail-closed: after AVFoundation finalizes the file, an
independent AVAssetReader pass must decode every video sample, confirm strictly
advancing timestamps, exact 1920x1080 geometry, bounded duration, and a useful
delivered-frame count. A file that fails validation is removed and is never
printed as an artifact. Errors identify the failing AVFoundation stage and
include its domain, code, reason, and underlying error where available. Do not
substitute FFmpeg, QuickTime UI automation, ScreenCaptureKit, OBS, or an ad-hoc
camera script; fix this single native path if capture fails. Stills and movies
are sink evidence, not replacements for the authoritative framebuffer capture.

Qualified-black platform candidates also require the attended evidence set in
`docs/bootstrap-black-qualification.md`. It covers cold reboot, active launcher
restart without RBF reload, game return, and injected preflight failure. Each
movie is retained beside Main events/status and latch status; none of these
physical checks may be replaced by `/dev/fb0` inspection.

`scripts/agent device display matrix --attended --out DIRECTORY [--usb-video]`
performs the bounded runtime display
matrix without rebooting Linux. Main applies each supported resolution as a
provisional transaction, the launcher restarts, and the authenticated MagiK
agent returns one PNG per mode. The command writes deterministic PNG names and
`manifest.json`, cancels every provisional mode after capture, and verifies the
original working mode is restored between cases. It requires an interactive
`31KHZ` acknowledgement because the matrix includes 480p/576p CRT modes, then
checks launcher PID replacement, output/framebuffer geometry, advancing frame
counters, RGB565 stride, nonblank content, and unique capture hashes. The v3
manifest records partial failures before cleanup. Framebuffer evidence still
requires attended sink observation for HDMI or CRT visibility claims.

With `--usb-video`, each case also captures the fixed `USB Video` input after
the authoritative framebuffer capture through the native agent capture command,
and records its path, size, and hash in the manifest. CRT/VGA acceptance
requires routing Morph 4K Port B before those cases and restoring HDMI afterward.
Morph credentials are runtime-only operator input and must never be passed as
command arguments or written to artifacts.

The launcher keeps output geometry one-to-one through 1280x720 and uses half
width and height when the output is at least 1366 pixels wide or 900 pixels
high. Custom modes use the same rule. The qualified ABI v3 hidden slots support
up to 1366x768 with a 2736-byte RGB565 stride and 2,101,248 bytes per slot.
