# Zero-copy scanout retirement

Date: 2026-07-11

Status: qualified and retired. Production remains cached RGB565 rendering into
two write-combined hidden scanout slots, posted through the FPGA vblank latch.

## Confirmed cause

The cacheable atomic scanout experiment did not remove presentation work. Its
cache synchronization cost replaced the legacy hidden-slot copy, while the
initial experimental timing stopped `copy_us` before cache synchronization and
started `post_us` afterwards. That omitted roughly 2 ms of cache clean work and
made the early result appear to save the whole copy cost. The corrected
qualifications include `hidden_compose_us` in work:

`prepare_us + slint_render_us + custom_draw_us + hidden_compose_us + fb_present_us`

The rejected implementation and its measured evidence remain in:

- [production baseline](2026-07-11-production-zero-copy-baseline.md)
- [production qualification](2026-07-11-production-zero-copy-qualification.md)
- [Arcade qualification](2026-07-11-arcade-zero-copy-qualification.md)
- [mailbox gate](2026-07-11-zero-copy-mailbox-gate.md)
- [Slint proof](2026-07-11-true-zero-copy-slint-proof.md)

## Removed interfaces

- FPGA command `0x59`, ACP atomic mailbox RTL, and mailbox testbench/CI test.
- `/dev/mister-magik-scanout`, the atomic UAPI/ioctls, DMA allocations and
  synchronization, cacheable mappings, ownership/fence state, PTE revocation,
  and mailbox bootstrap/status.
- Slint swapped-buffer atomic sessions, `MISTER_TRUE_ZERO_COPY`, atomic overlay
  targets, ownership transitions, and slot-alignment redraws.
- Agent/Desktop atomic ownership, mode, mailbox, and runtime-state fields.
- `--true-zero-copy`, its environment propagation, mailbox benchmark branches,
  the comparison helper, and the current mailbox ABI document.
- The experimental `plugin-probe` build/source naming and `dma_owned` metadata.

## Surviving production architecture

- Slint uses `RepaintBufferType::ReusedBuffer` and renders into cached RGB565
  memory.
- Dirty cached regions, preview composition, and Arcade-list composition are
  copied into alternating write-combined hidden slots.
- The FPGA `0x57/0x58` vblank latch posts the completed hidden slot.
- `mister_magik_scanout_slots.ko` exposes exactly two bounded write-combined
  mappings at `/dev/mister-magik-scanout-slots`, mode `0600`.
- Main and the Agent report `scanout_slots_module_loaded` /
  `scanout_slots_device_ready` and `scanout_slots: { module_loaded,
  device_ready }` respectively. The Desktop presents one readiness row.
- `/dev/fb0` remains the recovery fallback.

On-device mapping evidence reported two available regions, successful mappings
for both, `cache_mode=writecombine`, and `EINVAL` for both an invalid slot index
and an oversized mapping. The old devices/modules and all persistent destructive
reset arming files were absent after a normal reboot.

## FPGA build evidence

GitHub Actions run
[29153173239](https://github.com/NigelBreslaw/MiSTer-MagiK/actions/runs/29153173239)
completed successfully, including the Quartus latch-only RBF build.

- Artifact: `fpga-vblank-latch-build/menu-magik-vblank-latch.rbf`
- RBF SHA-256: `7f4f5c40260f52341f11f3cc66891c551699376dd89fc39ff03efdebd48eb5c2`
- Local qualification copy: `build/fpga-vblank-latch/menu-magik-vblank-latch.rbf`

## Matched 30-second qualification

Each scenario used two BEFORE and two AFTER runs through
`fpga-vblank-latch-hidden`. All four AFTER traces had zero latch deadline
misses, visual latch misses, buffer-alternation failures, FPGA drops, fallback
vsync sources, unsupported status, or wrong backends.

| Scenario / metric | BEFORE R1 / R2 | AFTER R1 / R2 | Matched median change |
| --- | ---: | ---: | ---: |
| Home p99 work | 6.892 / 6.929 ms | 6.931 / 6.912 ms | +0.16% |
| Home cached-present p95 | 1.458 / 1.463 ms | 1.470 / 1.484 ms | +1.13% |
| Home cached-present p99 | 1.492 / 1.512 ms | 1.500 / 1.521 ms | +0.57% |
| Home copied rows p95 / p99 | 448 / 448 | 448 / 448 | 0 rows |
| Arcade p99 work | 5.634 / 5.526 ms | 5.673 / 5.656 ms | +1.51% |
| Arcade list-present p95 | 1.245 / 1.244 ms | 1.268 / 1.279 ms | +2.33% |
| Arcade list-present p99 | 1.510 / 1.421 ms | 1.505 / 1.555 ms | +4.40% |
| Arcade fb-present p95 | 0.069 / 0.069 ms | 0.066 / 0.055 ms | -12.32% |
| Arcade fb-present p99 | 0.105 / 0.100 ms | 0.095 / 0.087 ms | -11.22% |
| Arcade copied rows p95 / p99 | 800 / 800 | 800 / 800 | 0 rows |

Arcade remained far below the 14.5 ms p99 work gate. AFTER Arcade R1 had one
stale preview-content sample among 1,800 frames; its pacing, latch, composition,
and backend gates passed, and R2 passed preview exactness with no invalid sample.
This did not affect the scanout retirement acceptance metrics.

Evidence paths:

- BEFORE Home: `build/launcher-home-scroll-profiles/ZC-RETIRE-BEFORE-HOME-R{1,2}-*`
- AFTER Home: `build/launcher-home-scroll-profiles/ZC-RETIRE-AFTER-HOME-R{1,2}-*`
- BEFORE Arcade: `build/arcade-scroll-profiles/ZC-RETIRE-BEFORE-ARCADE-R{1,2}-*`
- AFTER Arcade: `build/arcade-scroll-profiles/ZC-RETIRE-AFTER-ARCADE-R{1,2}-*`
- Kernel mapping report: device output from `scanout-slots-map-report`; the
  one-shot runner writes `build/scanout-slots/scanout-slots-map-report.log`.

## Commits

MiSTer MagiK:

- `6247d94` remove the FPGA atomic mailbox.
- `cffffbf` reduce the kernel component to two WC scanout slots.
- `4833dcc` restore the single cached Rust presentation contract.
- `f273ca9` replace atomic observability with slot readiness.
- `fa800b3` retire zero-copy benchmark/policy surfaces and experimental naming.

Main_MiSTer:

- `47eb8d9` update loader, live readiness status, tests, and patchset docs.
