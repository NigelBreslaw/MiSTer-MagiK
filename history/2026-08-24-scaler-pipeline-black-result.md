# Scaler-pipeline Phase 2 black result — 2026-08-24

## Result

The locally signed schema-5 diagnostic RBF reproduced the genuine persistent
MagiK black-screen failure on boot epoch 1, attempt 1. The campaign stopped
immediately. The device remains unrecovered: no subsequent transition,
launcher restart, RBF reload, or reboot has occurred.

The 1920×1080 native USB-video still is uniform video-level black: minimum,
maximum, and mean luma are all `16`. Its SHA-256 is
`dc4ee4f1eb9ede8c4b031b29fc8ba97d72068a296fb716cec2de087b6b25255e`,
byte-identical to the prior confirmed persistent-black incidents. The
authoritative 960×540 RGB565 framebuffer is complete and varied with SHA-256
`288f47335560f1169890ee50d02ddf3707ef4b568a22ccb06593a78d275ad250`,
again byte-identical to the known-correct Arcade return. A 30-second native
AVFoundation movie was recorded before recovery and remains ignored under the
artifact root.

## FPGA evidence

Failure-time and later live captures both classify the state as
`scaler_read_scheduler_stall`. The later capture contains three identical,
CRC-valid, coherent completed-frame samples with stable MagiK ownership and
`LauncherActive`:

- `o_readlev=2` and `o_copylev=2` in every sample;
- no accepted read and no current Avalon return in any observed frame;
- no nonzero returned word;
- completion request, destination observation, source acknowledgement, and
  pending state all idle and equal;
- zero retained return credits, zero return phase, and drain inactive;
- scaler running, with copy-read, line-write, and raw-active timing observed;
- no copied, line-written, or raw-output nonzero data observed.

The exact flags word is `0x0541` (`1345`) and the exact state word is `0x100a`
(`4106`) in all three samples. Latch-v5 continues posting with zero drops and
zero rejects, and the observer correctly reports
`sink_visibility: "unobserved"`.

## Interpretation and next decision

This rejects completion-queue backlog as the black-screen mechanism in this
incident. Both completion-toggle endpoints agree, pending is clear, and no
retained Avalon return credit exists. The scaler instead retains two pipeline
blocks, preventing another read from being admitted. Copy, line-write, and raw
timing still run, but no nonzero data progresses.

The next experiment should remain passive and narrower than schema 5. It needs
only enough completed-frame evidence to distinguish:

1. a missing copy/block retirement (`lev_dec` never occurs);
2. a copy state that remains active but never reaches its terminal condition;
3. an incorrect or stale buffer/metadata selection causing a zero copy to
   repeat without retiring.

This decision is implemented as schema 6, `scaler-copy-retirement-v1`. It
replaces schema 5 and records only the exact copy terminal predicate,
`lev_dec_v`, FSM progress, address wrap, copied-data nonzero, and front metadata
signature repetition/change. The frozen design is in
[`2026-08-24-scaler-copy-retirement-design.md`](2026-08-24-scaler-copy-retirement-design.md).

Do not alter the queued completion repair, latch-v5, routing, reset, PLL, mux,
or pixel output. Do not add a general observer. Once the failing retirement
condition is identified, replace diagnostics with the smallest functional
repair and rerun the local proof and physical campaign.

## Frozen identity

- host revision at preservation:
  `c27f40f00`;
- signed FPGA builder commit:
  `fac0a8fcf95fed4e10beb0814fb0fd3a6c8fe327`;
- Menu source commit:
  `3c3634c0105d78f27aeba66b38966c50dbc42c9b`;
- production patch SHA-256:
  `5d10c0148898b31612028d69720ce66246a383f40742e9373d30f668fafc228d`;
- RBF SHA-256:
  `00c37925ce4003449a3495647a43bcab7c2cd503803fe687294850c12fdc42c4`;
- metadata SHA-256:
  `c3c2fbd271c726753dcc2c53dd84aa06136a280e34bef3260ad92d63e45f9e73`;
- signoff report SHA-256:
  `3b3ef9f23dc041330c6d2096f61d9741c95b18515f53a06ce812c9084a8cf7a4`;
- local signoff: setup `0.427 ns`, hold `0.247 ns`, zero TNS;
- device agent version: `28`, protocol `2`;
- runtime launcher: `0.2.5219`, build `5219`, source
  `aa954e639ee3461c90f0420f9eed56ec00e6b637`;
- Main PID/generation: `9070` / `9003147`;
- launcher PID: `9093`;
- owner epoch: `1`.

Compact evidence is retained in
[`2026-08-24-scaler-pipeline-black-incident-v1.json`](2026-08-24-scaler-pipeline-black-incident-v1.json).
Large media remains ignored under
`build/scaler-pipeline-state-phase2-epoch1-01/`.
