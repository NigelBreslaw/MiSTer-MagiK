# MiSTer MagiK Agent

`mister-magik-agent` is a standalone MiSTer-side development backplane. It is
separate from the Slint/MagiK UI binary and is installed as an early init script:

```text
/etc/init.d/S00magik-agent
/media/fat/mister-magik/mister-magik-agent
```

The agent currently configures the static Ethernet path at boot and exposes a
token-protected line-delimited JSON control port on TCP `7498`. Zaparoo Core
uses TCP `7497`, so the MagiK agent must not bind that port.

## Protocol

The listener binds to `0.0.0.0:7498`. Each request is one JSON line:

```json
{"token":"...","id":1,"cmd":"status","args":{}}
```

Each response is one JSON line:

```json
{"id":1,"ok":true,"result":{}}
```

Failures use:

```json
{"id":1,"ok":false,"error":"..."}
```

The SD Card browser uses two compatible directory-listing commands. Legacy
`sd_list_dir` returns metadata-rich `mister-magik-sd-list-dir-v1` entries.
`sd_list_dir_v2` returns lightweight `mister-magik-sd-list-dir-v2` entries with
only `name`, `path`, and `kind`, allowing the desktop to render a sorted folder
without issuing one metadata lookup per item. New desktops try v2 once and
fall back to v1 when connected to an older agent.

The token lives on the MiSTer at:

```text
/media/fat/mister-magik/agent.token
```

The install script keeps a local copy for host tooling at:

```text
build/mister-agent.token
```

Do not commit the token. Host tools also accept `MISTER_AGENT_TOKEN` for
one-off overrides.

## Host Commands

Use the normal wrapper:

```bash
scripts/mister agent ping
scripts/mister agent status
scripts/mister agent logs
scripts/mister agent timeline
scripts/mister agent sd-list /_Arcade --protocol auto --repeat 5
scripts/mister agent diagnostics --out build/agent-diagnostics/sample
scripts/mister agent framebuffer-capture build/fb0.png --json build/fb0.json
scripts/mister agent deploy-magik-bin magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb
scripts/mister agent magik status
scripts/mister agent magik restart-launcher
scripts/mister agent reboot-wait --timeout 40
scripts/mister agent reboot-wait --direct-reset --timeout 40
scripts/mister agent boot-profile 3 --timeout 40
scripts/mister agent boot-profile 3 --timeout 60 --fail-on-timeout
```

`ping` confirms the authenticated TCP path. `status` returns:

- agent version, boot id, uptime, and port
- `eth0` carrier, operstate, IP, MAC, routes, ARP entries, RX/TX counters
- `sshd`, `MiSTer_MagiK`, and `mister-magik-fb` process ids
- system uptime

`logs` returns the in-memory ring buffer over the TCP agent protocol. The ring
keeps the newest 512 lines and reports how many older lines were dropped.

`framebuffer_stream_v1` is the desktop live-inspection path. The desktop sends
one authenticated JSON request on TCP `7498`, then the agent replies with a JSON
`ok` line and switches the same connection to the binary
`mister-magik-framebuffer-stream-v1` protocol. The agent proxies producer frames
from `mister-magik-fb` on local port `127.0.0.1:7499`; it does not poll or read
`/dev/fb0`. Frames are RGB565 little-endian keyframes or dirty rect deltas, LZ4
compressed per message, with heartbeat frames while idle.

The stream handshake identifies its source as the producer-side cached render
buffer. The agent never reads scanout slots or `/dev/fb0` to construct
steady-state frames.

The normal `status` response includes a `scanout_slots` object with two facts:
whether `mister_magik_scanout_slots` is loaded and whether
`/dev/mister-magik-scanout-slots` is ready.

`device_telemetry_stream_v1` is the lightweight desktop Real Time debug path.
The desktop sends one authenticated JSON request on TCP `7498`, then the agent
replies with a JSON `ok` line and keeps the connection in newline-delimited JSON
mode. It sends one `mister-magik-device-telemetry-v1` snapshot per second until
the desktop closes the socket. The stream samples `/proc` and `/sys`, reads the
current Slint status file from `/tmp/mister-magik/status.json` when present, and
does not read framebuffer pixels, query SQLite, walk media directories, or write
to `/media/fat`.

Each telemetry snapshot may be partial if the launcher or status file is not
available. The desktop should treat missing nested objects as unknown values,
not as stream failure. Current fields include:

- per-core and combined CPU busy percent from `/proc/stat`
- memory split as MagiK RSS, other used, and available
- MagiK/Main pids, RSS, and thread counts
- network RX/TX byte rates
- `/media/fat` capacity
- launcher screen, idle/FPS, preview cache state, and frame-budget aggregates

The desktop starts this stream only while the Debug page's `Real Time` tab is
active, and stops it when the user switches tabs or leaves the Debug page.

`framebuffer-capture` is the one-shot still-image path. It asks the MiSTer-side
agent to read the current framebuffer, convert the raw pixels to PNG on the ARM
device, and return the PNG plus metadata over the authenticated TCP protocol.
The host wrapper writes the PNG to `OUT.png`; `--json OUT.json` records
dimensions, stride, bpp, raw bytes, PNG bytes, request timing, and per-stage
capture/encode timings. Use it for evidence snapshots and benchmark artifacts,
not for desktop live streaming.

`timeline` returns structured boot events. The expected event names are:

- `agent_start`
- `control_listen`
- `ip_configured`
- `raw_arp_sent`
- `carrier_up`
- `first_tx`
- `first_rx`
- `sshd_seen`
- `magik_main_seen`
- `magik_launcher_seen`
- `first_client_connect`
- `first_command`

Each event has `uptime_ms`, `event`, and `detail`. Use the timeline to explain
boot samples without SSH and without scraping text logs.

`diagnostics` writes a local bundle directory. It tries the TCP agent first and
falls back to SSH when the agent port is unavailable. The bundle includes:

- `bundle.json`
- `status.json`
- `timeline.json`
- `agent-logs.json`
- `net.json`
- `processes.json` and `ps.txt`
- MagiK status files and recent Main/Slint/agent log tails
- `crashes.json` and `crash-latest.json` when local crash reports exist

Crash reports are local JSON files under:

```text
/media/fat/mister-magik/crashes/
```

`latest.json` is a copy of the newest report. Reports use schema
`mister-magik-crash-report-v1` and are written by either `mister-magik-fb` for
Rust panics or `MiSTer_MagiK` when the supervised launcher child exits
unexpectedly.

`reboot-wait` asks the agent to schedule a reboot, then waits for the agent port
first and SSH second. It defaults to the supervised MagiK visual-lockdown reboot.
The Main fork keeps OSD/menu/framebuffer paths suppressed, then asks Linux to
reboot through:

```sh
/sbin/reboot
```

The agent writes a synchronous `reboot_scheduled` breadcrumb to
`/media/fat/mister-magik/bootlogs/agent.log` before rebooting so a failed reboot
can still be root-caused after manual recovery. Pass `--raw` only for fallback
recovery or detached Linux reboot testing without MagiK visual lockdown.
Pass `--direct-reset` for fast dev-loop reboots after all writes have completed
and synced. Keep supervised reboot for settings/INI changes, release gates,
Ethernet soak tests, and unknown write state. `--direct-reset-no-sync` is only
for attended experiments.

`magik` exposes Main-owned launcher supervisor controls:

```bash
scripts/mister agent magik status
scripts/mister agent magik suspend
scripts/mister agent magik resume
scripts/mister agent magik restart-launcher
```

The control actions write `mister_magik_suspend`, `mister_magik_resume`, or
`mister_magik_restart_launcher` to `/dev/MiSTer_cmd` only when `MiSTer_MagiK` is
running. The agent does not directly kill or spawn the launcher. It logs the
command, before/after MagiK pids, pid changes, and errors into the RAM ring.
Responses return parsed Main and Slint status files plus current pids. The
`slint_status_current` flag is `false` when the status file belongs to an exited
launcher process, which is expected immediately after `suspend`.

`deploy-magik-bin` uploads a local `mister-magik-fb` binary over the agent TCP
port using a JSON header followed by payload bytes. The agent accepts raw bytes
or LZ4 block payloads. The host defaults to raw for small binaries and only
tries LZ4 automatically above `MISTER_AGENT_DEPLOY_COMPRESS_MIN_BYTES`
(default 8 MiB); set `MISTER_AGENT_DEPLOY_ENCODING=lz4-block` to force a
compressed test. The header includes original byte count, transmitted byte
count, encoding, and original FNV64 checksum. The agent receives the payload
into RAM, decompresses if needed, verifies the original bytes, asks Main to
suspend the launcher, writes a same-directory `.upload` file under
`/media/fat/mister-magik/`, renames it over the final binary, marks it
executable, then resumes the launcher. Existing `scripts/mister deploy-magik-bin`
remains the explicit SSH/SFTP fallback. `scripts/deploy-rust.sh` uses the agent
transport by default; set `MISTER_DEPLOY_TRANSPORT=ssh` only when intentionally
testing or recovering through the old path.

`boot-profile` reboots the device, waits for ports to drop, then compares first
agent response against first SSH command readiness. It defaults to the
supervised MagiK reboot path and accepts `--fail-on-timeout` for release gates
such as the three-reboot Ethernet soak. Pass `--raw` only when testing the detached
Linux reboot path. Pass `--direct-reset` for fast quiescent dev-loop reboot
samples after writes are complete; keep the supervised default for release soak
evidence. `--direct-reset-no-sync` is only for attended A/B testing. New rows include
`agent_rx_increasing` and `agent_rx_nonzero` in the note field so RX-zero or
non-increasing boots are not counted as recovered. Rows are appended to:

```text
history/toolchain-bench/results-agent.tsv
```

For shutdown-side attribution, install `scripts/mister-shutdown-trace.sh
install-deep` before running a reboot profile. It times each `rcK` service stop
plus `swapoff` and `umount`. Use `scripts/reboot-shutdown-summary.py` after
collecting logs to compare host timing, Main reboot breadcrumbs, shutdown trace
rows, and agent network health.

## Install And Recovery

Build and install:

```bash
scripts/build-mister-agent.sh
scripts/mister-magik-agent.sh install
```

Inspect:

```bash
scripts/mister-magik-agent.sh status
scripts/mister-magik-agent.sh log
```

The agent mirrors RAM log lines to `/tmp/mister-magik-agent.log` for compatibility
with older scripts. It delays SD-card persistence to
`/media/fat/mister-magik/bootlogs/agent.log` until after the boot hot path.

Remove:

```bash
scripts/mister-magik-agent.sh remove
```

SD-card recovery if SSH is lost: delete `/etc/init.d/S00magik-agent`. If the
legacy shell FastNet service is needed, rename
`/etc/init.d/disabled-S00fastnet.magik-agent` back to
`/etc/init.d/S00fastnet`.

## Timing Interpretation

The TCP control port can answer before SSH handshake/auth/exec completes, but it
still depends on the same Ethernet carrier and LAN neighbor path. It is a
foundation for faster diagnostics and deploy/control workflows; it is not a
kernel/PHY shortcut.
