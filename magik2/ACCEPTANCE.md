# MiSTer MagiK Tooling 2.0 acceptance record

This is a living evidence record for Document 2. An item is complete only when
the cited command and its retained result prove it; host compilation alone is
not device acceptance.

## Verified on the configured MiSTer

- Native bootstrap installed the separate agent on TCP 7500 and left the real
  MagiK binary, manifest, Main, and FPGA platform untouched.
- `scripts/magik2 deploy` starts the RGB565 probe after its readiness signal.
  The probe reports a 960x540 initial presentation.
- The unchanged warm path was sampled 20 times on 2026-09-05 with the viewer
  closed. Every run was a zero-byte no-op; invocation-to-completion samples
  were 727, 729, 733, 743, 745, 747, 748, 749, 755, 759, 760, 761, 764, 800,
  823, 829, 849, 849, 868, and 893 ms. Nearest-rank p95 is 849 ms, below the
  one-second requirement.
- The internal prebuilt-artifact path was sampled 20 times with distinct
  executable hashes and no compilation. Invocation-to-completion samples were
  2147, 2176, 2192, 2200, 2206, 2236, 2300, 2301, 2331, 2344, 2360, 2365,
  2399, 2399, 2475, 2481, 2502, 2594, 2606, and 2618 ms. The 11,957,000-byte
  uploads completed in roughly 1.5–2.0 seconds; nearest-rank p95 completion
  is 2606 ms, below the five-second requirement. One separate 11 MB upload
  timed out before acknowledgment and remains retained as failure evidence;
  it was excluded from the successful 20-sample set.
- Native observation delivered metrics, 100 recent probe logs, and a
  1,036,872-byte keyframe (72-byte wire header plus a 960x540 RGB565 surface)
  to the localhost viewer with no stream error. No framebuffer device polling
  was introduced.
- The agent advertises `agent-update-v1`, `artifacts-v1`, `lifecycle-v1`,
  `metrics-v1`, `status`, `test-bridge-v1`, `upload-v1`, and `watch-v1`; the current probe remains
  running after deploy and after a failed profiled-check setup.
- A native `agent-update-v1` replacement completed against the running service.
  The replacement acknowledged its SHA-256, retained the complete capability
  set, and adopted the already-running probe without a probe restart.
- Smoke passed through the isolated pinned Slint Python client and retained its
  screenshot. Motion passed through the same client with 1,800 confirmed
  scanout-latch posts and flips, zero physical drops, and a persistent-probe
  restart after the session.
- The profiled motion repetition retained a 17,211-byte folded stack file and
  a 37,549-byte flamegraph. The stacks contain both `mister_magik2_probe::main`
  and Slint software-renderer symbols.

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

- Measure viewer observation overhead,
  including slow/disconnected viewers.
- Measure 20 invocation-to-completion runs for Rust-edit and Slint-edit cases.
  Retain all samples, bytes, throughput, and nearest-rank p95; do not
  substitute phase timing for those targets.
- Exercise failure, lost-reply, deadline, concurrent-stream, and cleanup
  matrices; retain both primary and cleanup outcomes.
