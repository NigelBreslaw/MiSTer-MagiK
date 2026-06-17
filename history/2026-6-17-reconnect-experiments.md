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

Follow-up root-cause pass: a later supervised pair produced one good sample
(`13.223s`) and one host-down timeout. The preserved basic boot logs showed
FastSSHD had started sshd and FastNet had reached carrier in the failed window,
so the failure is likely after early sshd/fastnet startup rather than an sshd
launch failure. To catch the next failure, FastNet/FastSSHD were updated to
append persistent logs under `/media/fat/mister-magik/bootlogs/` with boot IDs
and 40s post-carrier/sshd snapshots.

After adding persistent snapshots:

- 16.140s SSH command-ready
- 13.306s SSH command-ready
- 13.254s SSH command-ready

The inspected successful run had early sshd listening around 5.6s, carrier up
around 9.7s, route/IP stable after carrier, and the sshd listener alive through
the post-carrier snapshots. The extra logging adds some drag but gives SD-card
recoverable evidence if the intermittent host-down failure recurs.

Manual reboot follow-up after the intermittent timeout:

- User power/manual reboot came back cleanly when checked after the fact.
- Current boot log: FastSSHD started at 3.70s, sshd started at 4.93s, carrier
  was observed up at 7.98s.
- Fresh supervised sample with 40s timeout: TCP/22 at 12.789s, SSH command-ready
  at 13.385s, MiSTer-reported uptime 10.99s at command execution.
- Matching boot log: FastSSHD started at 4.40s, sshd started at 5.63s, carrier
  was observed up at 9.83s.

Conclusion: FastSSHD is not the current limiter on successful boots. The sshd
daemon is alive several seconds before the host can connect. The dominant
remaining delay is Ethernet carrier/link readiness and host visibility after
carrier, so the next serious experiment should target PHY/link negotiation or
Mac-side neighbor discovery, one variable at a time.

## Experiment 8 - Restrict FastNet Link Advertisement To 100 Full

Hypothesis: gigabit autonegotiation might be the remaining carrier delay. If
FastNet restricts eth0 advertisement to 100baseT/full with autoneg still on, the
link might settle closer to early sshd readiness.

Fresh before samples with FastNet + FastSSHD:

- 13.376s SSH command-ready
- 13.594s SSH command-ready
- Average: 13.485s

Live probe:

- `ethtool -s eth0 advertise 0x008 autoneg on` succeeded.
- The live link renegotiated to 100Mb/s full and SSH stayed reachable.

After boot-time FastNet advertisement samples:

- 13.371s SSH command-ready
- 13.431s SSH command-ready
- Average: 13.401s

Device-side carrier timing moved only modestly. Before boots had carrier-ready
around 9.84s and 9.38s; after boots had carrier-ready around 9.40s and 8.63s.
However host TCP/22 visibility remained around 12.8s and command-ready remained
around 13.4s. Stock networking later restored the normal gigabit/all-modes
advertisement, so the tweak was also not a durable link-mode policy.

Result: rejected. The boot-time 100/full advertisement was removed and the
known-good FastNet service was reinstalled. The remaining delay is not explained
by gigabit mode selection alone.

## Experiment 9 - Move FastNet Earlier To S00

Hypothesis: FastNet was starting after syslog/klogd as `/etc/init.d/S03fastnet`.
Moving the same background worker to `/etc/init.d/S00fastnet` might let it wait
for eth0 and start link/IP setup earlier without changing its network behavior.

Fresh before sample after restoring normal FastNet:

- 13.549s SSH command-ready
- TCP/22 visible at 13.023s
- FastNet worker start 4.49s, carrier-ready 10.26s

After samples with `S00fastnet`:

- 13.290s SSH command-ready
- 13.541s SSH command-ready
- 13.261s SSH command-ready
- Average: 13.364s

Device-side after timing:

- Worker start: 4.08s, 3.79s, 4.06s
- Carrier-ready: 9.00s, 9.25s, 9.16s

Result: small keeper. This is not the large breakthrough, but it starts FastNet
slightly earlier and consistently moves carrier-ready earlier than the immediate
pre-change boot. The host-visible command-ready improvement is only around
0.1-0.2s, so the next limiter appears to be host visibility or post-carrier
network/SSH readiness rather than only FastNet launch order.

## Experiment 10 - Post-Carrier ARP Announcement Burst

Hypothesis: the Mac might be losing time on neighbor discovery after carrier.
FastNet already sends `arping -A` during each configure pass, but adding a burst
of unsolicited and answer-style ARP announcements immediately after carrier
could make TCP/22 visible earlier.

Fresh before sample with clean `S00fastnet`:

- 13.242s SSH command-ready
- TCP/22 visible at 12.769s

After samples with post-carrier ARP burst:

- 13.255s SSH command-ready
- 13.657s SSH command-ready
- Average: 13.456s

Logs showed that `arping -c 1` takes roughly a second per packet on this MiSTer,
so the burst added several seconds of background FastNet delay and did not move
host TCP/22 visibility earlier. The clean `S00fastnet` service was reinstalled.

Result: rejected. Neighbor announcement is not the current answer in this form;
if revisited, it needs a nonblocking raw packet sender rather than repeated
`arping` processes.

## Experiment 11 - Defer Persistent Boot Logs Off The Hot Path

Hypothesis: FastNet and FastSSHD persistent diagnostics were writing many small
lines to `/media/fat` during the reconnect window. Since `/media/fat` is slow
exFAT/FUSE, those writes could add jitter and delay. Keep detailed `/tmp` logs
during boot, but defer the SD-card copy until about 20s uptime.

Fresh before sample with clean `S00fastnet`:

- 15.819s SSH command-ready
- TCP/22 visible at 15.173s

After samples with deferred persistent logging:

- 13.217s SSH command-ready
- 13.417s SSH command-ready
- 13.574s SSH command-ready
- Average: 13.403s

Device-side inspected boot after the change:

- FastNet worker start: 3.47s
- FastSSHD start: 3.62s
- FastSSHD sshd started: 4.52s
- FastNet carrier-ready: 7.88s
- FastSSHD saw carrier up: 8.33s

Deferred persistence was verified:

- FastNet copied logs to SD at about 23.51s uptime.
- FastSSHD copied logs to SD at about 23.66s uptime.

Result: keeper. This removes slow SD-card writes from the reconnect hot path and
keeps delayed persistent evidence. It does not break the host-visible 13s floor,
but it lowers device-side carrier timing and avoids the observed 15.8s logging
drag. The remaining gap is now clearly after carrier/sshd readiness: the device
has sshd alive around 4.5s and carrier around 8s, while the Mac still sees
TCP/22 around 12.7s.
