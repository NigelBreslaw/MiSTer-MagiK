# MiSTer MagiK Tooling 2.0 acceptance record

This is a living evidence record for Document 2. An item is complete only when
the cited command and its retained result prove it; host compilation alone is
not device acceptance.

## Verified on the configured MiSTer

- Native bootstrap installed the separate agent on TCP 7500 and left the real
  MagiK binary, manifest, Main, and FPGA platform untouched.
- `scripts/magik2 deploy` starts the RGB565 probe after its readiness signal.
  The probe reports a 960x540 initial presentation.
- The unchanged warm path was sampled 20 times on 2026-09-05. Every run was a
  zero-byte no-op; recorded native-connect samples were 508–698 ms, with
  nearest-rank p95 615 ms. This is phase timing, **not yet** the required
  invocation-to-completion p95.
- Native observation delivered metrics, probe logs, and a 1,036,872-byte
  keyframe (72-byte wire header plus a 960x540 RGB565 surface). No framebuffer
  device polling was introduced.
- The agent advertises `artifacts-v1`, `lifecycle-v1`, `metrics-v1`, `status`,
  `test-bridge-v1`, `upload-v1`, and `watch-v1`; the current probe remains
  running after deploy and after a failed profiled-check setup.

## Automated evidence

- 20 focused host tests cover wire framing, hash publication, cached builds,
  frame decoding, result bundles, capability compatibility A→B→A, absent-agent
  bootstrap, missing-capability upgrade, authentication failure, and command
  dispatch.
- 5 focused native-agent tests cover framing, truncation, hash verification,
  atomic publication, and authenticated loopback upload.
- The probe and agent build for `armv7-unknown-linux-gnueabihf`; the probe links
  pprof revision `431b88c4fc67bef98126eaa8932287583d1a660e`.

## Remaining acceptance gates

- The pinned `slint-testing==0.3` package index currently returns HTTP 401.
  Until valid credentials are configured, smoke, motion, and profile sessions
  cannot run through the required isolated host environment.
- Run and retain smoke and motion evidence through the Slint framework. Motion
  must retain valid physical-presentation evidence with zero physical drops;
  software timings and vsync counters are deliberately insufficient.
- Verify the browser viewer itself and measure its observation overhead,
  including slow/disconnected viewers.
- Produce and inspect an on-device ten-second profile with nonzero useful
  probe/rendering symbols, folded stacks, and a flamegraph.
- Measure 20 invocation-to-completion runs for unchanged, changed-prebuilt,
  Rust-edit, and Slint-edit cases. Retain all samples, bytes, throughput, and
  nearest-rank p95; do not substitute phase timing for those targets.
- Exercise failure, lost-reply, deadline, concurrent-stream, and cleanup
  matrices; retain both primary and cleanup outcomes.
