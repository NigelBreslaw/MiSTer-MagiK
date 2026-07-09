# 2026-07-08 - Experiment C: FPGA Vblank-Latched Buffer Register

## Summary

Experiment C starts the FPGA-side path for fixing launcher tearing/frame pacing:
instead of asking Linux/Main to time a buffer flip, add an experimental FPGA
command that queues framebuffer route metadata and latches it on HDMI vblank.

2026-07-09 correction: the initial command IDs and activation path below were
superseded by device evidence. The live protocol now uses `0x57` for set and
`0x58` for status because `0x43` collides with stock Menu and `0x53..0x56` are
file-I/O commands. The patched launcher RBF must be activated from the MagiK
launcher with `mister_magik_launch <rbf>` and proven through Main's cmdline
before interpreting latch support reports; `load_core <rbf>` from the launcher
state left stock Menu active.

This slice did not replace the Menu core on the device. The Menu RTL checkout in
this repo is read-only reference material, so the safe first step was:

- add a real patch artifact for the Menu core;
- add a MagiK hardware diagnostic for the new command protocol;
- run the diagnostic on the current unpatched core to prove the probe is
  harmless and to record the baseline;
- restore normal MagiK afterward.

## Artifacts

- `experiments/fpga-vblank-latch/Menu_MiSTer-vblank-latched-fbuf.patch`
- `scripts/fpga-vblank-latch-one-shot.sh`
- `build/fpga-vblank-latch/fpga-latch-report.log`

The patch applies cleanly to `reference/Menu_MiSTer`:

```bash
git -C reference/Menu_MiSTer apply --check ../../experiments/fpga-vblank-latch/Menu_MiSTer-vblank-latched-fbuf.patch
```

## Proposed FPGA Protocol

New UIO commands:

- `0x43` - `MAGIK_UIO_SET_FBUF_LATCH`
- `0x45` - `MAGIK_UIO_GET_FBUF_LATCH` (`0x44` is now used by upstream Menu)

`0x43` accepts the same framebuffer route payload shape as `UIO_SET_FBUF`:

1. enable/filter/format
2. base low
3. base high
4. width
5. height
6. hmin
7. hmax
8. vmin
9. vmax
10. stride
11. optional sequence

The key difference is that the command writes pending `MAGIK_LFB_*` registers.
The active `LFB_*` registers are updated only on the synchronized HDMI vblank
rising edge.

`0x45` returns status counters and active route metadata:

- active sequence
- pending sequence
- pending/enable flags
- flip count
- post count
- drop count
- active base
- active width/height/stride

## Device Baseline

Command:

```bash
scripts/fpga-vblank-latch-one-shot.sh
```

Current deployed Menu core result:

```text
fpga_latch_set_probe_tsv	cmd=0x43	supported=0	magic_expected=0x4d47	ack_high=0x0000	ack_low=0x0000	error=
fpga_latch_status_tsv	cmd=0x44	supported=0	magic_expected=0x4d48	ack_high=0x0001	ack_low=0x0001	active_sequence=0	pending_sequence=0	pending=0	pending_enabled=0	active_enabled=0	flip_count=0	post_count=0	drop_count=0	active_base=0x00000000	active_width=0	active_height=0	active_stride=0
```

Interpretation: the current core does not implement the MagiK latch protocol.
That is expected. The probe did not hang the UIO bus, did not reboot the device,
and did not alter the framebuffer route.

## Restore And Hygiene

The runner restored the normal non-diagnostics MagiK binary, restarted the
Main-supervised launcher, and verified:

- `MiSTer_MagiK` running;
- `mister-magik-fb` running;
- no stale `launcher.env`, fs-fault files, or `rebuild-on-next-boot` marker.

Post-run status showed:

```text
MiSTer_MagiK    pid=612
mister-magik-fb pid=6001
fb_mode:        565 1 960 540 1920
```

## Result

This is a partial execution of Experiment C:

- FPGA source change is prepared as an applyable patch.
- Rust/device diagnostics for detecting the patched core are implemented.
- Current hardware baseline is recorded.
- The actual patched FPGA core has not been built or deployed from this
  workspace yet.

## Next Step

Use a writable/buildable `Menu_MiSTer` checkout to build a one-shot patched
`menu.rbf`, then boot it in an attended session and rerun:

```bash
scripts/fpga-vblank-latch-one-shot.sh
```

Required first success condition:

```text
fpga_latch_set_probe_tsv supported=1 magic_expected=0x4d47
fpga_latch_status_tsv supported=1 magic_expected=0x4d48
```

Only after that should we add a posting diagnostic that writes alternating
hidden-slot framebuffer routes and verifies `flip_count`/`active_sequence`
advances at vblank.
