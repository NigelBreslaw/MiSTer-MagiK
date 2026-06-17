# MiSTer MagiK Agent

`mister-magik-agent` is a standalone MiSTer-side development backplane. It is
separate from the Slint/MagiK UI binary and is installed as an early init script:

```text
/etc/init.d/S00magik-agent
/media/fat/mister-magik/mister-magik-agent
```

The agent currently configures the static Ethernet path at boot and exposes a
token-protected line-delimited JSON control port on TCP `7497`.

## Protocol

The listener binds to `0.0.0.0:7497`. Each request is one JSON line:

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
scripts/mister agent diagnostics --out build/agent-diagnostics/sample
scripts/mister agent deploy-magik-bin magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb
scripts/mister agent magik status
scripts/mister agent magik restart-launcher
scripts/mister agent reboot-wait --timeout 40
scripts/mister agent boot-profile 3 --timeout 40
scripts/mister agent boot-profile 1 --supervised --timeout 40
```

`ping` confirms the authenticated TCP path. `status` returns:

- agent version, boot id, uptime, and port
- `eth0` carrier, operstate, IP, MAC, routes, ARP entries, RX/TX counters
- `sshd`, `MiSTer_MagiK`, and `mister-magik-fb` process ids
- system uptime

`logs` returns the in-memory ring buffer over the TCP agent protocol. The ring
keeps the newest 512 lines and reports how many older lines were dropped.

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

`reboot-wait` asks the agent to schedule a reboot, then waits for the agent port
first and SSH second. It defaults to the same detached Linux reboot primitive as
the host wrapper:

```sh
nohup /sbin/reboot >/dev/null 2>&1 &
```

The agent writes a synchronous `reboot_scheduled` breadcrumb to
`/media/fat/mister-magik/bootlogs/agent.log` before rebooting so a failed reboot
can still be root-caused after manual recovery. Pass `--supervised` only for
explicit MagiK visual-lockdown reset validation.

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
port using a JSON header followed by raw payload bytes. The host includes byte
count and FNV64 checksum. The agent verifies the payload in RAM, asks Main to
suspend the launcher, writes a same-directory `.upload` file under
`/media/fat/mister-magik/`, renames it over the final binary, marks it
executable, then resumes the launcher. Existing `scripts/mister deploy-magik-bin`
remains the SSH/SFTP fallback.

`boot-profile` reboots the device, waits for ports to drop, then compares first
agent response against first SSH command readiness. It defaults to Linux
`/sbin/reboot`; pass `--supervised` only when validating the MagiK visual
lockdown reset path. Rows are appended to:

```text
history/toolchain-bench/results-agent.tsv
```

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
