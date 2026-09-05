# MiSTer MagiK Tooling 2.0 acceptance record

This is a living evidence record for Document 2. An item is complete only when
the cited command and its retained result prove it; host compilation alone is
not device acceptance.

## Verified on the configured MiSTer

- Native bootstrap installed the separate agent on TCP 7500 and left the real
  MagiK binary, manifest, Main, and FPGA platform untouched.
- `scripts/magik2 deploy` starts the RGB565 probe after its readiness signal.
  The probe reports a 960x540 initial presentation.
- `scripts/magik2 stop` stopped the owned probe and restored Main's ordinary
  launcher; a subsequent unchanged deploy restarted the probe successfully.
- A deliberately invalid prebuilt artifact produced a retained
  `start-failed; launcher-recovery=passed` result; the valid probe was then
  redeployed successfully.
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
- A one-line Slint edit was sampled 20 times, then the source was restored and
  rebuilt normally. Invocation-to-completion samples were 11,009, 10,213,
  10,195, 10,946, 11,320, 11,007, 10,896, 10,434, 10,064, 9,927, 9,772,
  9,969, 9,829, 9,785, 9,994, 9,911, 9,874, 9,702, 10,960, and 9,844 ms.
  The retained bundles contain each build, upload-byte, upload-elapsed, start,
  and completion event; nearest-rank p95 is 11,009 ms, below 15 seconds.
- A one-line Rust edit was sampled separately 20 times, then the source was
  restored and rebuilt normally. Invocation-to-completion samples were 13,150,
  13,358, 13,769, 12,665, 13,605, 13,630, 11,587, 10,669, 11,622, 10,118,
  10,325, 10,008, 10,330, 10,745, 10,776, 10,008, 10,496, 9,628, 10,224,
  and 11,338 ms. The same retained per-phase byte/timing evidence gives a
  nearest-rank p95 of 13,630 ms, below 15 seconds.
- Current deploy bundles record upload bytes, elapsed time, and calculated
  bytes/second together. The source-edit runs predate that explicit derived
  field but retain the corresponding byte and elapsed values in every bundle.
- Native observation delivered metrics, 100 recent probe logs, and a
  1,036,872-byte keyframe (72-byte wire header plus a 960x540 RGB565 surface)
  to the localhost viewer with no stream error. No framebuffer device polling
  was introduced.
- The agent advertises `agent-update-v1`, `artifacts-v1`, `lifecycle-v1`,
  `legacy-isolation-v1`, `metrics-v1`, `request-replay-v1`, `status`,
  `test-bridge-v1`, `test-deadline-v1`, `test-deadline-v2`, `upload-v1`, and
  `watch-v1`; the
  current probe remains running after deploy and after a failed profiled-check
  setup. Its typed status reports `legacy_agent_running=False`, so normal 2.0
  operation is verified while the retired device agent is stopped.
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
- With an active localhost viewer consuming native frames, metrics, and logs,
  motion retained 1,802 confirmed latch flips and zero physical drops in
  33.3 seconds. The comparable viewer-closed run retained 1,800 flips in
  30.7 seconds; observation overhead is measurable but did not introduce drops.
- Five native watch attach/disconnect cycles each received telemetry and left
  the persistent probe healthy with zero physical drops.
- After the deadline correction, a fresh current-device motion session retained
  `build/magik2-results/20260905T105830Z-2f0a2eaf8fe3`: 30,915 ms elapsed,
  1,802 latch posts and flips, zero physical drops, zero vsync misses, and a
  successful persistent restart and cleanup.

## Automated evidence

- 26 focused host tests cover wire framing, hash publication, cached builds,
  frame decoding, result bundles, capability compatibility A→B→A, absent-agent
  bootstrap, missing-capability upgrade, authentication failure, command
  dispatch, lost-reply request-identifier reuse, and primary/cleanup failure
  retention.
- 11 focused native-agent tests cover framing, truncation, hash verification,
  atomic publication, authenticated loopback upload, replay-safe mutation
  responses, the absolute deadline under continuous session traffic, and a
  blocked watch stream that still permits a separate control response. The
  relay tests also cover client disconnect and application-exit paths.
- The probe and agent build for `armv7-unknown-linux-gnueabihf`; the probe links
  pprof revision `431b88c4fc67bef98126eaa8932287583d1a660e`.

## Failure and streaming matrices

- The lost-reply test reconnects once with the same request identifier; the
  native replay cache returns the original mutation result without publishing
  the upload twice. Reusing that identifier for different request content is
  rejected.
- Test sessions hold the mutation lane, use a 20-second application-connect
  limit and a 60-second absolute deadline from session start, and stop the
  owned test process on application failure, client
  disconnect, or deadline. The deadline test uses continuous traffic, so it
  cannot be extended by read activity.
- Control mutations are serialized while native watch streams remain on their
  own connections. Watch writes have a 500 ms bound, retain only the newest
  preview, and device motion with an active viewer retained zero physical drops.
- A deliberately induced smoke failure plus failed persistent restart retains
  distinct primary `check` and cleanup events and fails the command.
