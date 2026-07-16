# C05 deterministic media-contention contract

Benchmark class: correctness-only. No performance claim.

Confirmed cause: the contention runner suppressed only the generic benchmark
gate, so Arcade settling serialized pack starts. After all requested packs
terminated, worker `Done` still depended on catalog validation sending
`Finish`; continuous contention prevented that path, and low-memory handling
dropped the only media receiver.

Before:

- Parent: `8c166c8bb90f06a54e479257e220ec13588fb5bb`
- Command: `scripts/profile-media-arcade-contention.sh
  C05R-MEDIA-8C166C8-BEFORE --deploy-device --replace-label --secs 30
  --timeout 120`
- Result: invalid timeout with incomplete worker completion after 120 seconds.
- Raw: `build/media-cold-boot/C05R-MEDIA-8C166C8-BEFORE.report.tsv`

After mitigation 2 and review-driven scope correction:

- Command: `scripts/profile-media-arcade-contention.sh
  C05R-MEDIA-8C166C8-AFTER4 --deploy-device --replace-label --secs 30
  --timeout 120 --correctness-only`
- Four requested, queued, terminal, and successful packs; one worker `Done`;
  zero pack or worker failures.
- Worker auto-finished after 504 ms quiescence at 27.683 seconds.
- 1,119 overlapping frames: 581 download and 538 publish frames.
- All overlap frames used `fpga-vblank-latch-hidden` with `ok` status.
- Cleanup and final correctness validity passed.
- Performance, preview, thread, and pacing diagnostics remain recorded for item
  9 and are intentionally non-blocking for this correctness-only contract.
- Raw: `build/media-cold-boot/C05R-MEDIA-8C166C8-AFTER4.report.tsv`

Review-driven corrections:

- Low-memory progress no longer claims a rendered media row while the popup is
  suppressed.
- Retaining the worker during low-memory is limited to the volatile
  `bench-tools` contention mode; production still drops it.
- Worker auto-finish and launcher contention suppression now share the same
  `bench-tools` feature boundary.
- Two cfg-gated Home-trace test fixture fields repair a latent item-3 omission
  exposed by compiling the mandatory bench-tools test target. They have no
  runtime behavior and are required for that target to compile.

Validation:

- Media contention and cold-boot script self-tests.
- Targeted screenshot-media session tests: 11 passed.
- Bench-tools launcher frame-accounting tests: 12 passed.
- `scripts/dev-rust test`: 283 passed.
- `scripts/dev-rust check`: passed.
- GUI library clippy: passed with `-D warnings`.
- Host-tool clippy: passed with `-D warnings`.

Review:

- Independent reviewer: `/root/review_item5_after4`.
- Reviewed code diff:
  `47d9920813336a3702e881d8d775ad7d54b2fc8cdec88bd4443661a72bc2e698`.
- Reviewed evidence:
  `dc48dc7ed4e46f5db3061feb5885cd6d28658abe90effa9848bca8db965101eb`.
- Result: approved with no unresolved actionable findings.
