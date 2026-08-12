# Passive FPGA video diagnostics

The Phase 2 recorder localizes rare black, white, corrupt-pixel, and timing
failures without changing the framebuffer latch, scaler, SDRAM data path,
reset path, clock selection, or final video outputs. It is evidence collection,
not a health verdict or an automatic fix.

## Checkpoints

The RBF retains three independent first-fault records until the FPGA is
reconfigured:

- `0x5D` records complete, partial, restarted, and owned legacy `0x2F`
  transactions and the surrounding route/control state in `clk_sys`.
- `0x5E` records accepted `vbuf_*` addresses, saturated accepted-burst and
  returned-beat counts, timeout/reset faults, and no-read frame intervals in `clk_100m`.
  It does not connect to `vbuf_readdata` or `vbuf_writedata`.
- `0x5F` records frame, line, and active-line totals and all-black/all-white state
  from the already-registered `hdmi_out_*` signals in `hdmi_tx_clk`. It does
  not hash, copy, or modify pixels.

The observer arms only after an accepted MiSTer MagiK route and two owned
vblanks. The first fault freezes its native domain and requests stable mailbox
snapshots from the other domains. A missing clock produces a partial snapshot
after 4,096 `clk_sys` cycles. Reads never clear or mutate evidence.

The diagnostic ABI is separate from latch protocol v5. Existing commands
`0x57`–`0x5C`, capability bits, responses, ownership, and apply priority remain
unchanged. Every diagnostic response has its own magic, fixed length, schema,
and CRC-16/CCITT-FALSE value.

Schema 4 keeps the complete legacy payload and route geometry in a compact
41-word control history, with compact 16-word Avalon and HDMI event records.
The control record uses a 16-bit vblank epoch and saturating 8-bit lifetime
event counters; host monotonic timestamps retain the wider collection timeline.
The native records retain the first/last actual address, accepted/returned
accounting, route generation, reference timing, and fault flags without
carrying generic trace history or pixel-rate counters.

Schema 4 preserves schema 3's record sizes and layout while correcting the
lock signal's meaning: both the control and HDMI records now observe the real
`pll_hdmi_0002.locked` output exported through `pll_hdmi`, not the unrelated
adjustment-PLL LED status. A transient unlock that clears before the observer
arms is ignored; any unlock sampled after arming is retained as a control/clock
fault.

## Collection

Use the authenticated support bundle after a failure, before restarting or
reposting:

```text
scripts/agent device diagnostics --out PATH
```

The device agent reads control → Avalon → HDMI → control while holding the
existing FPGA UIO advisory lock. It retries the complete set once for CRC or
generation instability, checks Main's FPGA-owner epoch before and after, and
writes `fpga-video-diagnostics.json` beside the existing bundle members. SSH
fallback explicitly reports the record unavailable; it never reads raw UIO.

Do not poll these commands continuously. The Rust launcher presenter uses the
same advisory lock for its complete latch transaction; legacy Main UIO paths
do not. Collection is therefore restricted to a stable `LauncherActive`
ownership interval, and owner-epoch, launcher-state, or confirmed latch-owner
changes invalidate the snapshot.

Classifications are deliberately conservative: `legacy_control`,
`avalon_no_reads`, `avalon_stall_or_return`, `final_black`, `final_white`,
`final_timing`, `control_or_clock`, `partial`, or `unclassified`. They identify
where retained evidence first disagreed; they do not prove a repair, HDMI sink
health, or SDRAM data correctness.

## Release gates

The FPGA fast gate checks the separate generated contract, all native-domain
fault-injection simulations, the complete 41-word control response, final response
priority, immutable latch hashes, exact pinned Menu integration, passive cone
boundaries, and explicit synchronizer identification. The real PLL status uses
forced first-stage identification because Quartus otherwise treats that status
as clock-related. Its synchronized `clk_sys` value then crosses to the HDMI
observer through the normal two-register CDC chain. Native diagnostic records
are held immutable before acknowledgement, their generation is sampled twice
in `clk_sys`, and the complete payload is covered by a nonempty 8 ns net-delay
constraint. The output generation, output route-context, and fault-trigger
bundles also have an 8 ns skew bound terminating only at their destination
registers' data pins; clock-enable and readout logic is deliberately outside
that coherence check. Quartus 17 reports no max-skew paths for the two Avalon
bundles in this pinned design, so those remain guarded by the complete 8 ns
net-delay bound plus consecutive stable-generation sampling in the receiver.
The asynchronous clock groups exclude the bundled paths from ordinary
functional setup/hold analysis while leaving those explicit CDC bounds
effective. The synthesis
workflow applies the same analysis-only exclusive-to-asynchronous clock-group
change to stock, pre-observer, and final work trees so TimeQuest analyzes those
skew constraints without biasing the observer delta. No generated clock or
functional RTL is changed.

The matched seed-1 Quartus signoff builds stock Menu, the exact pre-observer
latch revision pinned by `video-diagnostics-baseline.commit`, and the final
diagnostic RBF. Functional warning, constraint-identity, and synchronizer drift
remain stock-versus-final checks. Observer overhead is final relative to the
pre-observer build: no added unconstrained output paths, no more than 0.15 ns
slack degradation, no more than 1,100 ALMs or 1,500 registers, and no added DSP
or block-memory use. The final build must also have zero TNS and at least 0.20
ns setup and hold slack. Production deployment of a changed RBF still requires
normal release qualification. A locally signed coherent artifact set may be
installed only in the Dev layout through the typed attended experimental FPGA
installer; it is neither production-deployable nor release-qualified.

On Apple Silicon, `scripts/agent fpga signoff` runs that same three-way build
and unchanged checker in Apple containers. It resolves the local `main` ref in
an isolated generated checkout and caches stock, pre-observer, and patched
synthesis independently. Observer RTL changes rebuild only the patched variant;
the invariant references are reused. Each replacement is built in a staging
directory and promoted only after it completes, so cancellation or failure
cannot destroy an existing valid cache. A failing checker report is retained
for fast reruns. Local Rosetta synthesis is a development signoff lane only;
GitHub still owns publishable platform RBFs.

Internal `ascal.vhd` probes, SDRAM data inspection, BRAM history, SignalTap,
writable diagnostics, and pixel-rate CRCs are explicitly outside Phase 2. They
require a new evidence-backed plan.
