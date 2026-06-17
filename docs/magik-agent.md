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
scripts/mister agent boot-profile 3 --timeout 40
```

`ping` confirms the authenticated TCP path. `status` returns:

- agent version, boot id, uptime, and port
- `eth0` carrier, operstate, IP, MAC, routes, ARP entries, RX/TX counters
- `sshd`, `MiSTer_MagiK`, and `mister-magik-fb` process ids
- system uptime

`boot-profile` reboots the device, waits for ports to drop, then compares first
agent response against first SSH command readiness. Rows are appended to:

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
