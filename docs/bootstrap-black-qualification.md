# Qualified Black Bootstrap

This is the acceptance contract for any candidate that changes Main launcher
startup, the MagiK Menu RBF, framebuffer routing, or latch ownership. Main, the
RBF, and the scanout module must come from one exact platform bundle. The
candidate tuple additionally binds the exact MiSTer MagiK app commit and Rust
runtime binary installed with that bundle. Stock root `menu.rbf` is not
modified.

## Required transition

Every cold boot, game return, resume, active restart, and crash respawn must
follow this order:

```text
MagiK Menu RBF native black
→ Main canonical LFB disable
→ runtime latch/platform preflight
→ FPGA ownership transfer
→ supervised child spawn
→ first verified latch-presented MagiK frame
```

Main may enter black only while it owns the FPGA and no launcher child exists.
The transition disables OSD, OSD keys, and launcher input. It must not write
the Linux framebuffer mode, clear `/dev/fb0`, enable a legacy framebuffer, or
transfer ownership before preflight completes.

Failure is closed: an unsupported framebuffer command, failed preflight,
ownership failure, or fork failure starts no child, restores Main ownership,
and shows stock OSD/input over the MagiK Menu RBF's native black background.

## Host and build evidence

The platform candidate must retain:

- bootstrap-black RTL simulation proving
  RGB is zero for every tested input while DE, HS, and VS pass unchanged;
- the pinned Menu integration check proving both HDMI and analog OSD mixers
  remain downstream of the native black stage;
- a clean Quartus build and timing report produced by the GitHub-only
  `Build MiSTer MagiK Platform` workflow for the MagiK RBF;
- Main host results proving black before preflight, preflight before ownership,
  ownership before spawn, and convergence of initial/resume/restart/crash paths;
- Main failure results covering unsupported framebuffer disable, preflight
  failure, and fork failure with no child and stock-OSD recovery.

## Attended device evidence

Record four continuous 60-second 1920x1080 at 30 fps `USB Video` movies:

1. supervised cold reboot;
2. active launcher restart without reloading the RBF;
3. game-to-launcher return;
4. qualification-owned injected preflight failure.

Start each movie before triggering the typed, attended operation. Use only
repository-owned `mister` commands or the attended release workflow for device
mutation; do not use raw SSH. The preflight fault must be volatile,
self-cleaning, bounded to the qualification session, and must be verified
cleared before the next case.

For each movie retain, in one evidence directory:

- the `.mov` artifact at the `USB Video` device's native 30 fps ceiling;
- `/tmp/mister-magik/events.jsonl`;
- `/tmp/mister-magik/main-status.json`;
- latch readiness/status and the exact platform candidate identity;
- the operator's frame-by-frame review result.

After the first post-transition black frame, every frame must remain black
until the first sustained MagiK frame. Menu static, OSD residue, terminal
content, partially rendered UI, or a nonblack transient fails the candidate.
The injected-preflight case must instead reach a stable stock OSD over black
with no MagiK child.

Any Main, RBF, scanout, runtime, manifest, or candidate identity change
invalidates all four movies. After they pass, rerun the complete attended
`scripts/agent release qualify` gate and retain its existing zero
drop/rejection and latch-stress evidence. That gate is operator-only and is not
run by an agent without explicit authorization.
