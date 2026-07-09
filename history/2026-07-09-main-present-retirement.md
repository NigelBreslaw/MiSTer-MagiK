# 2026-07-09 Main Present Retirement

The Main-mediated present experiments were removed from the live MagiK and
Main_MiSTer code paths.

Retired paths:

- `main-flip-v1`
- `main-vsync-hidden`
- `plugin-main-vsync-hidden`
- `mister_magik_present_flip_v1`
- `mister_magik_present_vsync_v1`
- `/tmp/mister-magik/present-request-v1`
- `/tmp/mister-magik/present-ack-v1`

Reason: the experiments proved useful facts but not a viable renderer. The
request/ack and FIFO paths either blocked the UI frame on Main's vblank wait or
kept present ownership split across processes. The project now keeps the
original `/dev/fb0` dirty-copy path as the fallback and keeps the plugin-backed
`fpga-vblank-latch-hidden` path as the surviving hidden-buffer experiment.

Evidence remains in:

- `history/2026-07-08-main-owned-present-prototype.md`
- `history/2026-07-08-main-owned-present-feasibility.md`
- `history/2026-07-08-plugin-main-present-experiment.md`
- `history/2026-07-08-plugin-presenter-thread-experiment.md`
