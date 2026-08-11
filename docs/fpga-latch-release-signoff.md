# FPGA latch release signoff (superseded protocol-v2 artifact)

## Disposition

This records evidence for the superseded pre-Menu-20260603 RBF. It is **not an
approval of the protocol-v3 production artifact**. Automated RTL and Quartus custom-delta signoff pass, and matched
single-device 960x540 qualification passes. The release remains blocked by the
open items below. The new hash requires complete host, Quartus, device,
update_all-overwrite, return, and rollback qualification.

## Candidate identity

- RBF SHA-256: `69e0e312b226c004bfe7fced2cc1145954efa1110cee7a0f58de1528d52627a1`
- Qualified MagiK source: `4e08fb4e8125f865d10167d4c9d3fd87815f4f11`
- Menu source: `cf4dfdee516fcaa6952bdd9fb47154e96c28567e`
- Patch SHA-256: `4bdd2bcee724bb988ab6a975c2532ccc39a4e2b5686fac6fe4c88528f9c55ba6`
- Latch RTL SHA-256: `b810de3fdffbe79b8496e7eaa3967b07f6aa70a3d78dabb41c6428d72d994b1a`
- Toolchain: Quartus 17.0.0 Build 595, seed 1
- Matched Quartus workflow: run `29158797784`
- Passing fast RTL workflow: run `29159218946`

The final manifest-producing workflow must reproduce this RBF hash from the
reviewed final source before tagging. A different RBF hash invalidates the
device evidence in this record.

## Automated evidence

- Icarus exercises LATCH-001 through LATCH-008 with bounded waits. Verilator
  lint, pinned-patch integration, opcode ownership, and complete reachable
  custom-RTL line coverage pass without exclusions.
- The matched Quartus delta reports `valid=1`: 153 normalized warning records
  in both collected report sets, setup slack 0.324 ns, hold slack 0.250 ns,
  TNS 0, 168 stock versus 166 patched unconstrained output paths, and one added
  recognized synchronizer chain with calculable MTBF. The flow-summary warning
  baseline and waiver policy remain in
  [Menu FPGA Warning Waiver Ledger](fpga-menu-warning-waivers.md).
- The release manifest binds the RBF, pinned source, patch, RTL, toolchain,
  seed, workflow URL, and every retained Quartus report. Missing, modified,
  dirty-source, unsigned-delta, and mailbox-era fixtures are rejected.

## Device evidence

The primary device is the known MiSTer at the repository's documented device
address. All matched samples used the same current MagiK binary, including the
preserved launcher subtitle alpha-animation change.

| Gate | Baseline median p99 work | Candidate median p99 work | 3% limit | Result |
| --- | ---: | ---: | ---: | --- |
| Home, 960x540 | 6799.5 us | 6788.0 us | 7003.5 us | pass |
| Arcade/preview, 960x540 | 5282.0 us | 5374.5 us | 5440.5 us | pass |

All four candidate samples report zero latch deadline misses, visual latch
misses, buffer-alternation failures, sampled flip gaps, and unexpected FPGA
drops, with advancing post and flip counters. Deliberate faster-than-vblank
posting increments the drop counter; normal-cadence posting subsequently
recovers with no pending request. Reloading the same exact RBF clears the test
counters before protocol-integrity gates. These zero-drop results do not prove
zero dropped frames: physical-refresh cadence and dropped-frame counts are a
separate qualification axis, and motion requires exactly zero dropped frames.

A physical USB HDMI capture was recorded at 1920x1080 from the candidate. Its
contact strips show the expected moving Home row without an obvious horizontal
discontinuity. The retained raw capture is intentionally outside git under
`build/launcher-home-pan-captures/FPGA-LATCH-CANDIDATE-HDMI-USB-20260711.mov`.
This is primary-device evidence, not the required independent second-display
review.

## Open release gates

- **1280x720 hidden slots:** each production hidden slot exposes 1,040,384
  bytes, while a 1280x720 RGB565 frame needs 1,843,200 bytes. The renderer
  correctly falls back to `/dev/fb0`, but the hidden-latch geometry gate is not
  satisfied. The analyzer now fails an expected-latch backend mismatch instead
  of incorrectly reporting it valid.
- **Game-return lifecycle:** the bounded smoke twice failed to create
  `/tmp/mister-magik/launcher-return-state.json`. Main and the launcher remained
  healthy with zero crash/invariant counts. The runner now removes persistent
  `launcher.env` on every exit, but this lifecycle requirement is unresolved.
- **Two-hour soak:** explicitly deferred; not run in this qualification session.
- **Hardware breadth:** no second representative MiSTer/display combination has
  been qualified.
- **Independent review:** review of the extracted RTL, Menu integration,
  constraints, waiver ledger, and retained evidence is not yet recorded.

## Installation verification and rollback

For rollback audit only, run:

```text
scripts/checks/verify-fpga-rbf-manifest.py --historical-v2 build/fpga-vblank-latch/menu-magik-vblank-latch.metadata.txt
```

Deployment uploads the RBF and adjacent metadata to temporary names, verifies
the exact hash, and only then activates the pair. Boot installation refuses a
missing or mismatched pair. After activation, `fpga-latch-report` must show
`0x57`/`0x58` support and advancing counters for the expected RBF command line.

Rollback with `scripts/agent device mode set stock --attended`. For a one-shot candidate rollback
without changing boot policy, launch the retained stock Menu RBF through Main's
normal command path; do not use an external `rbf_load` while the launcher is
active.

## Release checklist

- [x] Latch-only requirements and requirement-to-test matrix
- [x] Extracted RTL and reduced Menu integration patch
- [x] Complete reachable RTL and functional requirement coverage
- [x] Passing matched stock, pre-observer, and final Quartus delta
- [x] Exact-RBF manifest and deployment verification
- [x] Matched primary-device 960x540 performance and latch integrity
- [x] Deliberate overflow and recovery
- [x] Primary-device HDMI capture
- [ ] 1280x720 hidden-latch geometry
- [ ] Game-return lifecycle smoke
- [ ] Two-hour motion soak
- [ ] Second representative MiSTer/display
- [ ] Independent release review

Release tagging is prohibited while any item above remains unchecked.
