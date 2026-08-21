# Black-screen recurrence on repair-only platform-v0.29

## Incident

On 2026-08-21 the operator reported a physically black HDMI screen while the
framebuffer still contained the real MiSTer MagiK application. The device was
left in the failing state. No USB video, framebuffer capture, reboot, RBF
reload, launcher restart, display-mode change, or other mutation was performed.

Two typed read-only snapshots were collected with:

```text
scripts/agent device diagnostics --out build/black-screen-20260821-live
scripts/agent device diagnostics --out build/black-screen-20260821-live-2
```

The exact FPGA records are retained alongside this note:

- `2026-08-21-black-screen-recurrence-fpga-1.json`
- `2026-08-21-black-screen-recurrence-fpga-2.json`

## Installed identity

The persisted installed-platform identity in the diagnostic bundle reported:

- release: `platform-v0.29`
- RBF SHA-256:
  `7484e004b3c6e089d9d377658633e435703bc1a224943b06215df9a9bccef4e7`
- manifest SHA-256:
  `87d0fd7c8314b5f5154d06122bd28a7ba9ca42fdd0aec3d3149490d61257f215`
- bundle ID:
  `67c943bddf3325f82d6e6666f6046b16dab9d5a972295b0167054b181443170e`
- qualification candidate ID:
  `b83a3a0b696b3cbe7cc6331c4ff49fbb1a8ba1bda4e1c7670ef67e0dd0f79105`
- platform source revision:
  `836b123ec4010c1aeec821272b6730425c06a5ce`
- Main revision: `f290719e97f5a3c84efa8e24691b80673b93f23c`
- Menu revision: `3c3634c0105d78f27aeba66b38966c50dbc42c9b`

The active launcher was build `4996`, version `0.2.4996`, from clean revision
`6c32becf2030c4b092b0fe07176f4a864a396f1d`.

## Preserved runtime state

At the first snapshot:

- Main PID `581`, generation `8030`; launcher PID `615`;
- `launcher_state=LauncherActive`, `fpga_owner=magik`, owner epoch `1`;
- framebuffer mode `565 1 960 600 1920`;
- zero crashes, restarts, invariants, blocked SPI writes, and blocked GPO writes;
- launcher scene `home`, HDMI route, 960x600 framebuffer;
- `present_backend=fpga-vblank-latch-hidden`, `present_status=ok`;
- `display_frozen=false`, latch sequence/flip count `2624`, drop count `0`;
- input enabled and source state revision `6`.

The active physical scanout base was `0x22fd2000` (`587014144`), width `960`,
height `600`, and stride `1920` bytes.

## Diagnostic delta

The captures were `91.42` seconds apart in device monotonic time.

| Field | First | Second | Delta |
| --- | ---: | ---: | ---: |
| owned vblank count | 17,940 | 23,420 | +5,480 |
| presented vblank count | 2,624 | 2,626 | +2 |
| repeated vblank count | 15,316 | 20,794 | +5,478 |
| active sequence | 2,624 | 2,626 | +2 |
| post count | 2,624 | 2,626 | +2 |
| flip count | 2,624 | 2,626 | +2 |
| drop count | 0 | 0 | 0 |
| reject count | 0 | 0 | 0 |
| ownership loss count | 0 | 0 | 0 |

The owned-vblank delta is approximately `59.94 Hz`. Owner epoch, MagiK
ownership, active base, protocol identity, and lifetime invariant all remained
stable. The two new source frames posted and flipped successfully while the
operator continued to observe black physical output.

This excludes the application source, active-slot selection, latch post/flip
transport, ownership, and stopped HDMI vblank as the immediate fault boundary.
The result is consistent with the earlier scaler fetch scheduler/return-credit
starvation boundary recorded in
`history/2026-08-14-fpga-video-diagnostics-design-attempts.md`, but it does not
prove the same internal state.

## Diagnostic limitation discovered

Both FPGA records report:

```text
diagnostic_architecture=scaler-completion-repair-v1
classification=repair_transport_ready
passive_video_observer=false
sink_visibility=unobserved
```

The installed production RBF does not contain the earlier passive video
observer or commands `0x60` through `0x67`. The control, Avalon, and final-output
diagnostic source files are empty compatibility stubs. In the device agent,
`repair_transport_ready` is derived only from stable ownership and valid latch
presentation telemetry. It does not observe scaler requests, accepts, returns,
completion queue state, raw scaler RGB, final mux provenance, or final RGB.

Therefore this preserved incident establishes a recurrence downstream of the
source/latch boundary and rejects the candidate's physical-output claim, but
the exact failing scaler or pixel-path mechanism cannot be recovered from this
RBF after reboot.

## Local source provenance

The original ignored diagnostic outputs had these SHA-256 digests before the
operator rebooted:

| Snapshot | File | SHA-256 |
| --- | --- | --- |
| first | `fpga-video-diagnostics.json` | `234696f51e62d2e45f8233e0ba9e243d0c53c565871adcadac51466f69fefa11` |
| first | `main-status.json` | `a6590d1befbf55999d19409b9fd02563bab34ed2b7a2d84a293a869604ea67c9` |
| first | `slint-status.json` | `0b75b284d1a4cce47179f3d7fb5c3fade7bdab1a98454d1915bbfda53d250a0e` |
| first | `bundle.json` | `c49c438b579e47ee5410cc50de5529056dedfa865414239d301f6b5dddd90f01` |
| second | `fpga-video-diagnostics.json` | `4d84dd5022df968b0d0f792b82586a71b4ff7a27ae10cb94e141e31691b5abd4` |
| second | `main-status.json` | `71dd9974e5efbe216aa3808f9688285dad237248d22bc0a7daa291bd027834ba` |
| second | `slint-status.json` | `18462b04a73177aae3b60b128106e04b063693d0827c31f763c876ed94b240e8` |
| second | `bundle.json` | `3c4a1ab7be684026686e07bc29fc2e5d23d893c4fecdf029f1e21ba6382752c0` |

The local `build/black-screen-20260821-live*` directories remain ignored and
must not be treated as checked-in evidence. The two adjacent FPGA JSON files
and this note are the curated, committed continuation boundary.
