# Scheduler and exFAT performance attribution — 2026-08-19

## Outcome

The new typed benchmarks completed three valid captures each on the same MiSTer boot and installed runtime. Every retained trace had zero overruns, the launcher was restored after each run, and the installed manifest and binary identity remained unchanged.

The scheduler route is healthy at the frame deadline: 350 measured frames produced no physical drops, maximum frame wall time was 16.592 ms, and the UI main thread's runnable-delay p99 was 56–62 us. The clearest scheduler-side cost is instead a shared USB interrupt arriving almost exactly 8,000 times per second on CPU0 and consuming 7.90–7.96% of capture wall time in its two handlers.

The catalog rebuild is both storage- and single-core-structure limited. The first, colder-metadata-cache pass took 233.3 s; the next two took 174.6 and 172.6 s. Directory walking alone fell from 93.5 s to 39.6/38.3 s after Linux cached exFAT metadata. The import phase remained nearly fixed at 77.1–78.0 s and reported one shard worker, no pipeline overlap, and a peak of one item in flight.

Machine-readable evidence is in [performance-attribution-20260819.tsv](performance-attribution-20260819.tsv) and [performance-attribution-20260819.json](performance-attribution-20260819.json). Large raw traces remain in the ignored `build/agent-benchmarks/` capture directories identified below; the tracked manifest records their SHA-256 hashes.

## Implementation ledger

### `a6f70ca14` — bounded scheduler trace benchmark

- [x] Add a typed `scheduler-trace` benchmark scenario.
- [x] Own an isolated tracefs instance and restore all state on success or failure.
- [x] Capture scheduler switches, wakeups, migrations, IRQs, and softirqs.
- [x] Correlate the fixed GUI route with per-thread, per-CPU, overlap, and interrupt summaries.
- [x] Validate installed identity, display restoration, launcher restoration, and trace integrity.

### `7ea2fdb8b` — isolated exFAT storage attribution

- [x] Add a typed `storage-attribution` benchmark scenario.
- [x] Resolve `/media/fat` to its exFAT mount and underlying MMC block device.
- [x] Redirect every writable catalog artifact to a bounded disposable Dev directory.
- [x] Sample `/proc/PID/io` and 11/15/17-field block statistics during a real catalog rebuild.
- [x] Capture block requests plus descendant process lifecycle and use process/block counters when syscall tracepoints are unavailable.
- [x] Inspect the generated catalog, prove the production registry is unchanged, then remove isolated output.

### Compatibility hardening

- [x] `c69a978fb`: raise the per-CPU trace buffer from 4 MiB to 16 MiB after an integrity failure exposed the device's IRQ volume.
- [x] `b3e6efb43`: use Main's acknowledged suspend contract so the isolated `library-refresh` owns the process-exclusive lock, then restore the launcher.
- [x] Preserve fail-closed behavior: the two compatibility probes retained diagnostics but did not count as evidence captures.

## Measurement conditions

| Property | Value |
|---|---|
| SoC | dual-core Cortex-A9 |
| Display | HDMI 1280×720 at 60 Hz |
| Storage | `/media/fat`, exFAT on `mmcblk0` |
| Mount options | `rw,noatime,nodiratime` |
| Installed MagiK revision | `2acb549bf627bff697af8fef6c2f689017dd59a4` |
| Boot ID | `320651eb-ca64-4e2c-87d5-ca270a05f726` |
| Trace buffer | 16 MiB per CPU |
| Scheduler captures | `1787120541`, `1787120590`, `1787120676` |
| Storage captures | `1787120840`, `1787121193`, `1787121486` |

The storage output directory was deleted after each capture, so each run rebuilt the output catalog. The kernel page cache was deliberately not dropped: this repository's typed workflow does not mutate global cache state. Consequently, run 1 is a colder metadata-cache observation and runs 2–3 describe the warm-cache floor, not three identically cold trials.

The device kernel exposes the required block and process-lifecycle tracepoints but not the requested `syscalls:*` tracepoints. Syscall attribution is therefore explicitly marked unavailable; `/proc/PID/io`, MMC block counters, block request tracepoints, and catalog phase markers provide the fallback evidence.

## Scheduler findings

| Metric | Run 1 | Run 2 | Run 3 | Interpretation |
|---|---:|---:|---:|---|
| CPU0 scheduled-task busy | 18.29% | 18.06% | 17.54% | Substantial task headroom remains. |
| CPU1 scheduled-task busy | 45.45% | 45.45% | 47.83% | UI/render work is concentrated here. |
| Both CPUs non-idle | 9.49% | 9.95% | 9.89% | Only about one tenth of wall time exploits both cores concurrently. |
| Shared USB IRQ rate | 7,998/s | 7,999/s | 7,998/s | Stable 8 kHz microframe-scale interrupt load on CPU0. |
| USB handler wall share | 7.94% | 7.90% | 7.96% | Measured combined time in the two handlers on IRQ 50. |
| UI main runnable p99 | 59 us | 56 us | 62 us | Normal scheduler latency is not the frame bottleneck. |
| UI main switches | 1,875/s | 1,866/s | 1,940/s | A high wake/switch rate remains worth attributing with pprof/PMU. |
| Maximum measured frame | 16.572 ms | 16.570 ms | 16.592 ms | Under the 16.667 ms physical frame period. |
| Physical dropped frames | 0 | 0 | 0 | Authoritative cadence passed in all captures. |

Scheduled-task busy time and interrupt-handler time are not additive: an interrupt can be nested inside a task's scheduled interval. The USB percentage is still a valid upper bound on CPU time reclaimable by eliminating that handler work.

CPU1 carried 45.45–47.83% scheduled-task busy time while CPU0 carried 17.54–18.29%; simultaneous non-idle time averaged only 9.78%. This does not justify relaxing UI real-time policy—the measured p99 is already small—but it does show room for deliberate phase-level parallel work on CPU0 without moving the latency-sensitive UI main/vsync path.

## Storage findings

| Metric | Colder run | Warm run 1 | Warm run 2 |
|---|---:|---:|---:|
| End-to-end sampled duration | 233.29 s | 174.63 s | 172.63 s |
| Directory walk | 93.50 s | 39.59 s | 38.27 s |
| Complete scan | 127.80 s | 70.40 s | 68.98 s |
| Import/persist | 77.98 s | 77.30 s | 77.11 s |
| Process physical reads | 199.47 MiB | 160.91 MiB | 158.77 MiB |
| Process physical writes | 174.83 MiB | 174.82 MiB | 174.74 MiB |
| Block request issues | 76,397 | 21,404 | 22,085 |
| Maximum MMC I/O in flight | 2 | 2 | 2 |
| Output catalog size | 134.00 MiB | 134.13 MiB | 134.00 MiB |

All runs traversed the same 160 targets, 10,571 directories, and 72,717 files. Warm metadata cut directory-walk time by 54.6/55.2 s and block-request count by roughly 71–72%, which explains essentially all end-to-end variation. The import phase was stable to within 0.9 s, so it is a separate deterministic optimization target rather than cache noise.

Within import, all three captures report:

- one shard worker, pinned catalog policy on CPU0;
- `pipeline_peak_in_flight=1` and `pipeline_overlap_us=0`;
- 32.1–32.5 s of shard-build wall time;
- 8.7–8.9 s of shard-publication wall time;
- 8.18–8.42 s copying and hashing 73,290,597 bytes during publication;
- 58.35–58.69 s in projection and about 5.3–5.4 s publishing the scanner cache.

The MMC write path is also consistent: about 169.5–169.7 MiB reaches the block device per rebuild, average write wait is 3.37–3.54 ms, and no more than two requests are in flight. Blindly adding writers could therefore shift a CPU bottleneck into exFAT queueing; the safe design is parallel construction with a bounded single publication queue.

## Optimization experiments, ordered by evidence

### 1. Two-core shard construction with serialized publication

Evidence: the rebuild suspends the GUI, yet catalog policy pins work to CPU0; every capture reports `shard_workers=1`, peak in-flight 1, and zero pipeline overlap. Shard construction alone costs 32.1–32.5 s, while CPU1 is available.

Idea: build independent system shards with two workers pinned one per Cortex-A9 core. Feed completed immutable shards to a capacity-one publication queue so exFAT writes and manifest ordering remain serialized. This uses the second core without multiplying filesystem writers.

Ceiling: ideal two-way construction saves about 16.2 s; overlapping the 8.7–8.9 s publication stage raises the measured upper bound to about 25 s. A practical first target is 12–20 s from the 77 s import phase.

Confidence: high for available parallelism, medium for realized gain. Risk: higher memory use, cache contention, and nondeterministic completion order. Smallest A/B: fixed warm-cache rebuilds with `shard_workers=1` versus 2, capacity-one publication, identical catalog hashes, RSS/HWM and MMC wait included as gates.

### 2. Hash while writing, then rename in place

Evidence: publication copies and hashes the same 73,290,597 bytes in 8.18–8.42 s on every run. That byte count is about 40% of the process's measured physical writes.

Idea: create each completed shard as a temporary file in its final exFAT directory, update the digest as bytes are written, fsync the file, atomically rename it, then fsync the directory/manifest boundary. This removes the second copy-and-hash pass while preserving transactional publication.

Ceiling: 8.4 s and 73.3 MB of redundant publication traffic per full rebuild. Confidence: high. Risk: recovery semantics on exFAT must be proven across interruption. Smallest A/B: apply only to the slowest `c64` shard, compare digest and catalog inspection, then run the existing bounded fault/recovery assurance before expanding to all shards.

### 3. Durable namespace snapshots for unchanged roots

Evidence: identical namespace traversal costs 93.5 s with colder metadata and 38.3–39.6 s warm; request issues fall from 76,397 to about 22,000 after cache warming. The traversal reads 10,571 directories and 72,717 files even though inputs are unchanged.

Idea: persist a compact per-target directory-entry snapshot after a successful build and reuse it when a target's validated root fingerprint is unchanged. Pair the fast path with explicit invalidation from updater/install flows and periodic or user-requested full verification. Do not rely on exFAT timestamps alone.

Ceiling: the observed colder-cache penalty is 54–55 s; eliminating the unchanged warm walk provides a further ceiling near 38–40 s. Confidence: medium because robust invalidation is the hard part. Risk: stale catalog entries after out-of-band SD-card edits. Smallest A/B: snapshot one large target, mutate add/remove/rename cases externally, and require exact discovery plus catalog-hash parity before enabling broader reuse.

### 4. Suppress unneeded 8 kHz DWC2 interrupts

Evidence: IRQ 50 enters the `ffb40000.usb` and `dwc2_hsotg:usb1` handlers 7,998–7,999 times per second on CPU0, consuming 7.90–7.96% of wall time in handler context. The rate is invariant across all three scheduler captures.

Idea: identify the DWC2 source bit and, when no isochronous endpoint requires microframe service, mask or coalesce the periodic source while retaining endpoint/event interrupts. Keep this an opt-in kernel experiment until controller and storage compatibility are established.

Ceiling: 7.9% of one Cortex-A9 core from measured handler time; any softirq reduction is additional but unproven. Confidence: high that the cost exists, medium that it can be removed safely. Risk: controller latency, USB audio/video, hubs, or mass-storage regressions. Smallest A/B: one guarded kernel parameter, the same controller route, USB device matrix, input-latency capture, and scheduler-trace comparison; revert automatically if devices disconnect or input deadlines regress.

## Priority and non-findings

Start with experiments 1 and 2: they are application-level, bounded, and directly target a stable 77 s phase. Experiment 3 has the largest cold-start ceiling but needs a trustworthy invalidation design. Experiment 4 can recover meaningful CPU headroom but belongs behind kernel/device compatibility qualification.

Do not spend the next optimization cycle changing UI scheduler priority. Across all three captures, normal runnable p99 stays below 62 us and every measured physical frame meets cadence. Use pprof and PMU next to explain the UI main thread's roughly 1,900 switches per second, but treat that as attribution work rather than evidence of a deadline failure.

## Evidence integrity

- Scheduler raw traces: `build/agent-benchmarks/scheduler-trace/{1787120541,1787120590,1787120676}/`.
- Storage raw traces: `build/agent-benchmarks/storage-attribution/{1787120840,1787121193,1787121486}/`.
- Each directory contains its generated `summary.json`, raw trace, trace stats, capability record, and derived tables/logs.
- Raw traces are intentionally ignored because of size; the tracked JSON manifest records identity and SHA-256 for every accepted trace.
- The earlier scheduler capture with a 4 MiB buffer and the earlier storage process-lock probe are excluded from all tables.
