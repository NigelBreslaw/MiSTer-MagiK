# MiSTer Reconnect Experiments - 2026-06-17

Goal: reduce the delay between MiSTer reboot and usable Mac-side development
connection. Measurements use `scripts/mister boot-net-profile` unless noted.

## Baseline Harness

- Supervised reboot baseline: SSH command-ready averaged about 19.0s across 3
  samples.
- Raw reboot baseline: SSH command-ready averaged about 23.9s across 2 samples.
- Already-booted connection profile: fresh SSH setup was about 0.23s; 4 MiB SFTP
  writes were about 9.6 MiB/s to `/tmp` and 5.1 MiB/s to `/media/fat`.

## Experiment 1 - Early `dhcpcd` From `inittab`

Hypothesis: starting `/sbin/dhcpcd -f /etc/dhcpcd.conf` immediately after
`/bin/mount -a` would bring the static wired IP up before the normal `rcS`
network sequence.

Change: `scripts/mister-fastnet-boot.sh install` adds this reversible line:

```text
::sysinit:/sbin/dhcpcd -f /etc/dhcpcd.conf & # mister-magik-fastnet
```

Fresh before samples:

- 20.906s SSH command-ready
- 20.557s SSH command-ready
- Average: 20.732s

After samples:

- 20.354s SSH command-ready
- 18.226s SSH command-ready
- Average: 19.290s

Result: not a convincing win. The boot log shows the early `dhcpcd` process
starts around 4s, but reports `no valid interfaces found`; the Ethernet driver
still appears around 9.5s and link-up around 13.7s. This suggests the remaining
delay is not just late `dhcpcd` startup. The device was restored with
`scripts/mister-fastnet-boot.sh remove` after the experiment.

## Experiment 2 - Start `dhcpcd` Right After `S10udev`

Hypothesis: experiment 1 was too early; starting `dhcpcd` as `S11dhcpcd` after
udev would let the wired interface exist sooner than the normal `S41dhcpcd`.

Change: `scripts/mister-early-dhcpcd-service.sh install` adds:

```text
/etc/init.d/S11dhcpcd -> S41dhcpcd
```

Fresh before samples:

- 20.687s SSH command-ready
- 20.528s SSH command-ready
- Average: 20.608s

After samples:

- 20.946s SSH command-ready
- 18.175s SSH command-ready
- Average: 19.561s

Result: not a convincing win. `dhcpcd` starts around 5s, but the Ethernet
driver still appears around 9.35s and link-up around 13.47s. Starting dhcpcd
earlier in init order does not pull the actual eth0 device initialization much
earlier. The device was restored with
`scripts/mister-early-dhcpcd-service.sh remove` after the experiment.

## Experiment 3 - Tighten Host `reboot-wait` Polling

Hypothesis: the host wait loop was losing time because each failed wait attempt
used a long TCP probe plus a 1s sleep. Tightening the probe to 150ms and polling
roughly four times per second should reduce the wrapper's own overhead.

Fresh before samples from `scripts/mister reboot-wait`:

- 18.2s wait-up time
- 15.7s wait-up time
- Average: 16.95s

After samples:

- 16.5s wait-up time
- 16.1s wait-up time
- Average: 16.30s

Result: small keeper. This does not change MiSTer boot/network readiness, but it
removes coarse host-side polling and trims about 0.65s from the two-sample
average while reducing worst-case miss time.

## Experiment 4 - Buffered Single-Session Binary Deploy

Hypothesis: development deploys waste time by opening several SSH sessions and
streaming the real binary through `io::copy` into SFTP. A single-session helper
plus a buffered `put()` should reduce the binary swap path.

Before, manually timing the existing deploy-style sequence for a 6,098,636-byte
`mister-magik-fb` binary:

- prepare lock: 0.164s
- suspend: 0.162s
- direct `/media/fat` SFTP put: 3.603s
- move/chmod/unlock: 0.153s
- resume: 0.138s
- final chmod/size: 0.154s
- Total: 4.374s

Intermediate finding: generated in-memory payloads of the same size wrote to
`/media/fat` in 1.108-1.501s, while the real binary file took 4.817-5.865s with
the old `io::copy` path. Reading the local file into memory and writing it with
one `write_all()` removed most of that gap.

After, `scripts/mister deploy-magik-bin`:

- 1.926s helper-internal total, 2.046s outer wall
- 1.902s helper-internal total, 2.017s outer wall
- Put phase: 1.656s and 1.629s

Result: keeper. The reviewed deploy swap path drops from about 4.374s to about
1.914s helper-internal average for this binary, and `scripts/deploy-rust.sh`
now uses the single-session helper for the main executable.

## Experiment 5 - Direct Static `eth0` Setup After Udev

Hypothesis: `dhcpcd` startup order was not enough because its interface
discovery path was slow. A tiny `S11staticeth0` service using direct `ifconfig`
and `route` might force the Ethernet device to initialize earlier.

Change: `scripts/mister-static-eth0-boot.sh install` adds
`/etc/init.d/S11staticeth0`:

```sh
/sbin/ifconfig eth0 192.168.1.117 netmask 255.255.255.0 up || exit 0
/sbin/route add default gw 192.168.1.1 eth0 2>/dev/null || true
```

Fresh before samples:

- 20.536s SSH command-ready
- 20.557s SSH command-ready
- Average: 20.547s

After samples:

- 16.135s SSH command-ready
- 18.200s SSH command-ready
- 15.844s SSH command-ready
- Average: 16.726s

Initial result: promising but later rejected as unstable. The Ethernet driver
moved from roughly 9.5s after kernel start to roughly 5.46s. Link-up still
happened around 13.7s in the inspected run, so physical link negotiation looked
like the cap, but the first sample set still cut about 3.8s from command-ready
average.

Follow-on failure: the next fresh before run for a force-100/full experiment
with `S11staticeth0` still installed never recovered SSH:

- `boot-net-profile` sample timed out after the device went down.
- `scripts/mister wait 60` timed out.
- ARP showed `192.168.1.117` as incomplete on the Mac.
- Unsandboxed `ping -c 4 192.168.1.117` had 100% packet loss.

Conclusion: do not keep direct static `ifconfig` boot setup as implemented. It
can pull Ethernet driver initialization earlier, but it can also strand the
device off-network. Remove it with `scripts/mister-static-eth0-boot.sh remove`
once SSH is available again, or delete `/etc/init.d/S11staticeth0` from the
MiSTer root image via offline recovery.

Recovery: after a manual reboot, SSH returned and
`scripts/mister-static-eth0-boot.sh remove` successfully removed the service.
`scripts/mister-static-eth0-boot.sh status` then reported
`static_eth0=not-installed`. The post-recovery boot log showed eth0 driver
initialization around 6.37s and link-up around 15.59s on the normal path.

## Experiment 6 - Background FastNet Agent

Hypothesis: the static `ifconfig` one-shot was unsafe because it fired once and
lost the race. A background agent that starts early, waits for `eth0`, repeatedly
applies static IPv4 setup, logs each attempt, and exits only after carrier
appears should pull network initialization forward without stranding the device.

Change: `scripts/mister-fastnet-agent.sh install` adds `/etc/init.d/S03fastnet`.
It starts before normal rcS network services and writes
`/tmp/mister-magik-fastnet.log`.

Fresh before samples:

- 20.638s SSH command-ready
- 22.777s SSH command-ready
- Average: 21.708s

After samples:

- 16.196s SSH command-ready
- 14.594s SSH command-ready
- 15.982s SSH command-ready
- Average: 15.591s

Result: keeper. The inspected boot moved Ethernet driver initialization to
roughly 3.79s, link-up to roughly 7.91s, sshd listening to roughly 11s, and
first SSH authentication to roughly 12s device uptime. This cuts about 6.1s from
the fresh command-ready average while surviving three measured reboots. The
agent remains installed for follow-on testing.

## Experiment 7 - Early FastSSHD And Manual Reboot Watch

Hypothesis: with FastNet active, stock sshd startup was the next visible delay.
Starting sshd from an early `/etc/init.d/S04fastsshd` service might allow the
Mac to connect closer to link-up.

Fresh before samples with FastNet active:

- 16.141s SSH command-ready
- 14.548s SSH command-ready
- Average: 15.345s

After samples:

- 18.120s SSH command-ready
- second sample never reached SSH readiness

At this point the result looked bad, but a manual reboot changed the picture.
The manual reboot boot log showed:

- FastSSHD started at 4.07s.
- FastSSHD started sshd at 5.16s.
- Ethernet link came up at 8.42s.
- First auth landed around 10s device uptime.

`scripts/mister watch-reboot --wait-down 180 --timeout 120` was added to time
external/manual reboots without issuing a reboot command itself. Manual reboot
with FastNet + FastSSHD:

- 12.503s SSH command-ready from observed down transition

Follow-up supervised samples with FastSSHD still installed:

- 13.332s SSH command-ready
- 13.438s SSH command-ready

Result: cautious keeper. The first supervised attempt had one bad timeout, but
manual reboot and two follow-up supervised reboots were materially faster than
FastNet alone. FastSSHD remains installed for continued testing. SD-card
recovery if it strands the device again: delete `/etc/init.d/S04fastsshd` from
the MiSTer Linux root image.
