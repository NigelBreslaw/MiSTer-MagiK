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
