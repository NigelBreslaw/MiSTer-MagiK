# Direct Desktop connection

The Rust Desktop connects directly to the native service on TCP 7500. Python is
used only by `scripts/magik desktop-prepare --json` to reuse discovery, Keychain,
credential migration, bootstrap and capability updates. Compatible service builds
remain installed. The preparation command returns identity/address/outcome, never
a token or SSH password; build progress and child-process output go to stderr.

## Read-only interface

All requests use the existing native length-prefixed envelope and authentication.
No new global wire version is introduced. Required features are `dashboard-status`,
`sd-browser`, `framebuffer-stream`, `telemetry-stream`, and existing identity,
status and authoritative capture support.

- `dashboard-status`: process/network state and current Main/application status.
- `sd-list`: `path` (SD-relative, default `/`) and `show_hidden` (default false).
  Returns the retained fast directory listing, with natural sorting.
- `sd-stat`, `sd-mra`, `sd-preview`: SD-relative `path`. Metadata/MRA/list results
  use a JSON binary body (`format: json`, operation `<request>-result`). Image
  preview metadata is in `sd-preview-result` with the original binary image body.
- `framebuffer-stream`: native `framebuffer-stream-ready` acknowledgement followed
  by unchanged RGB565/LZ4 producer frames. Source is explicitly
  `producer-pre-ownership-transfer`; it is not proof of displayed scanout.
- `telemetry-stream`: native ready acknowledgement, then `telemetry-sample` JSON
  bodies at one-second cadence. Includes timestamps, CPU, memory, processes,
  network, SD activity, launcher state and frame-phase measurements. Missing or
  initial delta measurements are null; measured zeroes remain valid values.
- `capture-framebuffer`: existing authoritative latched RGB565 capture. No preview
  or raw fb0 fallback. Desktop converts the same pixels for display/explicit save.

Requests with unsupported fields/body or inaccessible SD data return native
errors. Paths cannot escape the SD root through traversal or symlinks. MRA input
is bounded to 512 KiB, raw XML display to 256 KiB, image input to 16 MiB, and XML
row/depth limits report warnings. Oversized native results fail explicitly.

A stream has one native owner. Slow/disconnected consumers have bounded socket
writes; closing a framebuffer connection closes its producer connection. Telemetry
releases its own Analytics lease without deleting a newer owner's marker. No
subscription triggers app navigation, restart, display configuration or reboot.

## Desktop setup

Use the repository-launched Desktop. It shares `device.json` and identity-keyed
native token files under `MISTER_MAGIK2_STATE`, or
`${XDG_STATE_HOME:-~/.local/state}/mister-magik`. It verifies identity before
remembering a changed address. Rust discovery uses one eight-second local search;
no public networks are probed. `MISTER_IP` remains an optional explicit override.
For a genuinely ambiguous selection, use `scripts/magik device select ADDRESS`.
Keychain or authentication errors are reported; credentials are never cycled.

The client has one native preparation attempt and at most one transport reconnect
per stream. Leaving the view cancels it. Frames are reconstructed before UI
coalescing; telemetry and display updates use bounded latest-value mailboxes.
Standalone application packaging and old-agent uninstall remain separate work.

## Main-managed real application delivery

Real MagiK now requires `main-managed-magik`. A compatible native service stays
installed regardless of its build identifier. The service uses Dev Main's existing
`launcher.env` hook to select `/media/fat/mister-magik2/magik` and its tooling
settings, then resumes Main. It changes only a marked block in
`/media/fat/mister-magik-dev/launcher.env`; operator settings and production paths
are preserved. Main retains its startup handshake, input routing and FPGA ownership.
No Main_MiSTer change is needed. Its existing installed application still supplies
Main's platform preflight; the actual launched executable hash is checked separately.

Success requires the new process to publish a frame, the executable hash to match,
and Main to report the same PID active and ready, with FPGA owner `magik` and input
proxy capability. An independently spawned process while Main is suspended is no
longer healthy. Stopping or replacing a managed app suspends it through Main, never
signals the supervised child directly. Test/profile settings use the same marked
block; ending the session replaces them on normal restoration. Mini remains an
independent bounded workload. Failed starts report the Main state without rebooting
or silently retrying the deployment.
