# Document 2 — First-Version Implementation Plan

## Objective and user experience

Deliver a small working toolchain around a standalone Slint experiment. It must prove:

- Fast incremental build and native deployment.
- Native live metrics, logs, and framebuffer viewing.
- Correctness testing and benchmarking through one pytest-based system.
- Useful CPU sampling profiles of the same workload.
- Automatic device-agent preparation and capability-based compatibility.

The experiment remains running after deployment. An explicit stop command restores the ordinary launcher.

### Public commands

The thin entrypoint is `scripts/magik2`.

```text
scripts/magik2 deploy
scripts/magik2 check smoke
scripts/magik2 check motion
scripts/magik2 check motion --profile
scripts/magik2 watch
scripts/magik2 status
scripts/magik2 stop
```

- `deploy` builds when inputs changed, uploads when bytes changed, starts the app, and returns when ready.
- `check` ensures the artifact is available, runs the selected scenario, and writes one result bundle. With no scenario, run smoke and motion.
- `--profile` reruns the selected workload with CPU sampling enabled.
- `watch` opens a local browser viewer for metrics, logs, and frames.
- `status` reports the installed artifact, running process, and capabilities.
- `stop` stops the experiment and requests normal-launcher resume.

Use `MISTER_IP` for the device. Reuse existing SSH authentication inputs only for bootstrap. Keep the first release to one device and one active experimental application.

There is no separate benchmark command or benchmark scenario registry.

## Implementation decisions

### Independent project and fast builds

The isolated project contains the Python host, Rust agent, tiny Rust/Slint application, pytest scenarios, and a static browser viewer. It owns its lockfiles and scoped workflow instructions.

The host must not import or invoke the old host orchestration. The new device agent must not depend on the old agent-protocol or platform-manifest orchestration crates.

Use:

- Python with `uv`, pytest, the existing pinned `slint-testing` client, and host-side image/LZ4 support.
- A lazily imported SSH library for bootstrap only.
- Rust standard networking and threads for the small native service.
- Apple `container` for local ARM builds, using a dedicated cached build environment and persistent Cargo caches.
- The repository’s current Rust/Slint pins initially.
- One optimized probe build profile: optimization level 2, incremental compilation, no LTO, retained profiling symbols and frame pointers.
- Test introspection and profiling support compiled into that same probe binary. Sampling is inactive unless requested.

Enable the existing Slint system-testing metadata/fontconfig-loader settings required by the pinned client. The app event loop must service Slint callbacks, timers, and test requests.

Build reuse is based on relevant source inputs and artifact hashes. Do not embed a repository-wide commit counter or timestamp that forces rebuilding after unrelated changes. Record Git revision and dirty state in host results as context.

### The tiny application

Implement a standalone application with:

- A visible build label.
- A counter with increment/reset controls.
- A details panel that opens and closes.
- A deterministic motion workload containing 64 moving rectangles.
- Stable accessibility identifiers and explicit observable state.
- Device-side frame timings, presentation counters, and workload start/completion markers.

Use the existing installed display configuration. Do not add a display-mode matrix or modify persistent display settings.

Use Slint’s software renderer and the existing small cached RGB565 presenter through the minimal hardware-library feature boundary. Do not link the production app, catalog, media, FFmpeg, private assets, particle engine, or production UI generation.

The app emits readiness only after successful initial presentation. It reports software timings separately from validated physical presentation counters.

### Native service and compatibility

Run the new agent on a separate configurable native TCP port, default **7500**. Keep it separate from the existing agent and framebuffer producer.

Use one small control envelope: bounded JSON headers with request identifiers; binary bodies travel as length-delimited bytes. Bulk uploads must not use JSON/base64 or SSH. Keep streaming connections separate from control traffic.

The native interface covers:

- Capability/status discovery.
- Uploading an app or replacement agent.
- Starting and stopping the owned application.
- Following logs, telemetry, and frames.
- Bridging the Slint test connection.
- Retrieving named run artifacts.

Scenario names and test logic do not belong in the agent.

A handshake reports build identity and supported capabilities. Unknown optional response fields are ignored. Release/build numbers are informational and never compatibility gates.

For each operation:

1. Query the installed agent.
2. Proceed immediately if it supports the required capabilities.
3. Otherwise choose an available supporting agent artifact, building the local agent only if necessary.
4. Install it through the native connection, reconnect, verify capabilities, and continue.
5. Use SSH automatically only when the service is absent or cannot be repaired natively.

A suitable agent from another branch is retained. Do not require that its capabilities exactly equal the client’s set.

Keep application capabilities separate: replacing the agent cannot repair a missing feature in the app itself.

Authenticate native requests with a stored token. Bootstrap provisions or retrieves it using existing access; authentication failure must not silently rotate another installation’s credentials. Do not expose tokens in logs or artifacts.

### Deployment and process lifecycle

Keep all experimental installation files under `/media/fat/mister-magik2/`, with transient state under `/tmp/mister-magik2/`. Do not replace the real MagiK binary, its manifest, Main, or FPGA files.

Deployment performs:

1. Resolve/build the probe artifact.
2. Ensure the required agent capabilities.
3. Compare the app hash with installed state.
4. Stream changed bytes to a temporary file while calculating their hash.
5. Publish the complete verified file atomically.
6. Stop the previous owned experiment, suspend the ordinary launcher through Main, and start the new experiment.
7. Wait for readiness and report phase timings.

Upload while the old experiment remains running. An identical healthy application is a no-op; an identical stopped application starts without another upload.

The service owns the experimental child after the host disconnects. It tracks enough process identity to avoid acting on unrelated processes. Agent updates preserve or reconcile that running state.

On failed startup, retain logs, stop the failed owned process, and make one launcher-resume attempt. Report the original failure and recovery outcome separately. Do not reboot, restore platform snapshots, or claim successful recovery without evidence.

Mutating operations are serialized; streams remain usable. Reconcile ambiguous responses by request/application identity rather than blindly restarting an operation.

### One system for tests and benchmarks

Use the existing **pytest + Slint Python client approach**, with new small fixtures and a 2.0 transport adapter. Do not inherit the old build wrapper, device bridge, or arbitrary accessibility-tree-difference oracle.

A scenario consists of actions, explicit assertions, and optional measurement windows:

- **Smoke:** verify readiness/build label, increment and reset the counter, open and close the details panel, capture a screenshot.
- **Motion:** assert initial state, start the deterministic workload, measure it, assert completion/final state, and retain timing/presentation evidence.

The host prepares the native Slint bridge before starting the test app. Each check gets a fresh test session; it does not depend on reconnecting Slint’s one-shot test connection to an existing process. After the check, restart the same experiment in its ordinary persistent mode.

Use explicit expected values and panel states. Valid no-op actions must be testable. Release input and close sessions on exceptions.

For motion measurements:

- Use a two-second warm-up followed by five measured seconds.
- Run five unprofiled repetitions.
- Use device timestamps and frame counters.
- Keep accessibility queries and screenshots outside measured intervals.
- Report all samples, their distributions, display geometry, and evidence validity.
- Never substitute software FPS for physical dropped-frame evidence.

`--profile` adds a ten-second repetition using the existing ARM-compatible `pprof` implementation pinned to its resolved commit, sampling at 99 Hz. Produce folded stacks and an SVG flamegraph. Mark that repetition as instrumented; exclude it from unprofiled benchmark aggregates.

Functionality failures fail `check`. Performance results and unavailable metrics are explicit; they never prevent a later deployment. Missing required evidence cannot be presented as a passing assertion.

### Streaming and results

The app produces telemetry and framebuffer data; the agent forwards it. Do not introduce framebuffer-device polling.

Reuse the small existing framebuffer wire codec. Use bounded queues and discard obsolete preview frames for slow viewers. A viewer disconnect must not stop the app.

The new viewer is a localhost-only Python HTTP service with static HTML/JavaScript. It displays:

- Current application identity and state.
- Live timings and counters.
- Recent logs.
- A frame preview, capped at five updates per second initially.

Decode streamed frames on the host. The browser reads the latest cached preview; browser refreshes must not trigger device captures. Disable preview generation when nobody is watching.

Each command writes a unique result directory containing a small `run.json`, incremental events, logs, and any screenshots/profile artifacts. Record requested operation, app/agent identity, source context, timings, measurements, primary outcome, and cleanup outcome.

No workflow database, receipt hierarchy, or family of numbered report schemas is needed.

## Logical commits

Each commit must have focused tests and leave the project buildable. Intermediate commits may be incomplete features, but must not claim device acceptance prematurely.

| Commit | Implementation | Commit acceptance |
|---|---|---|
| **1. Establish the independent project and contract** | Add these documents, scoped instructions, Python/Rust project structure, thin entrypoint, ownership rules, dedicated CI selection, and tooling PR scope check | Host commands load without compiling Rust; old orchestration is not imported; experiment-only edits remain ordinary consumer changes |
| **2. Add cached builds and the minimal Slint probe** | Implement ARM build preparation/cache reuse, the small UI/event loop, RGB565 presentation, readiness, and local scenario controls | Agent/probe builds exclude the production app dependency graph; unrelated edits do not invalidate the probe; a source edit does |
| **3. Add the native service and bootstrap** | Implement authentication, capabilities, status, binary transfer, native self-update, and SSH-only bootstrap | Loopback tests cover absent/older/superset agents, truncated uploads, hashes, and capability-driven updates without version equality |
| **4. Complete deploy and stop** | Add Main suspend/resume adapter, owned-process lifecycle, readiness, no-op detection, restart reconciliation, and phase timing | Deploy leaves the probe running; stop restores the launcher; failed startup gives logs and a separate recovery result |
| **5. Add streaming and the new viewer** | Add producer telemetry/frames, native forwarding, bounded consumer queues, logs, and localhost viewer | Frames and metrics update live; slow/disconnected viewers cannot stall rendering or control requests |
| **6. Implement the shared pytest scenario runner** | Add Slint bridge, fixtures, smoke/motion scenarios, device-timed measurement windows, and result bundles | Same scenario performs correctness assertions and benchmark collection; failures retain evidence; sessions have absolute deadlines |
| **7. Add profiling to those scenarios** | Enable runtime-controlled CPU sampling, folded stacks, flamegraphs, and instrumented-result labeling | Motion profile has nonzero samples and meaningful application/rendering symbols; no alternate workload implementation or rebuild is required |
| **8. Prove the complete loop and document results** | Run branch-compatibility, failure, timing, streaming, and device acceptance matrices; fix measured bottlenecks; publish usage and results | All definition-of-done criteria below pass; unresolved misses are reported as unfinished work |

Commit messages should describe these concrete outcomes. Use dedicated tooling PRs; do not mix unrelated application changes into this work.

## Definition of done

### Functional proof on the MiSTer

- A single `deploy` bootstraps automatically if needed and visibly starts the probe.
- The probe remains running when deployment returns.
- An unchanged healthy deployment transfers nothing and does not restart it.
- `watch` shows live metrics, logs, and frames over native transport.
- Smoke and motion pass through the pinned Python Slint framework.
- Motion produces benchmark measurements from the device.
- Profiling produces a useful flamegraph from the same scenario.
- `stop` restores the normal launcher.
- The old agent CLI is never invoked, and normal operation works with the old device agent stopped.
- No production MagiK artifact or platform component is replaced.

### Speed acceptance

Measure 20 runs per warm-path case on the user’s Mac and MiSTer, with dependencies/build environment warm and the viewer closed. Use nearest-rank p95 and retain all samples.

| Case | Required p95 |
|---|---:|
| Unchanged `deploy`, invocation to completion | **≤1 second** |
| Changed prebuilt binary, deploy entry to visible readiness | **≤5 seconds** |
| One-line probe edit, including incremental build and deployment | **≤15 seconds** |

Measure Rust and Slint edits separately; both must meet the incremental target. Cold dependency compilation and first bootstrap are reported separately, without presenting them as warm iteration.

Add an internal prebuilt-artifact acceptance path so transfer/startup time can be measured independently of compilation. Record bytes transferred and throughput alongside stage timings.

A missed target prevents declaring this first version complete. It does **not** become a runtime deployment refusal.

### Compatibility and failure acceptance

Automated tests must demonstrate:

- A → B → A clients keep a suitable installed agent despite differing build identities.
- Missing required capabilities trigger one automatic native upgrade and continuation.
- Extra capabilities and optional fields do not cause rejection.
- An absent agent triggers bootstrap.
- Unrelated dirty files do not block or rebuild the probe.
- Truncated/corrupt uploads never become executable artifacts.
- Lost replies are reconciled without duplicate starts.
- App failure, client disconnect, and continuous traffic cannot defeat test-session deadlines.
- Primary and cleanup failures are both retained.
- Slow streams cannot block application progress.
- Authentication errors are actionable and do not trigger credential replacement.

For motion, valid physical presentation evidence must be retained and zero physical drops demonstrated on the test device. Also run with the viewer enabled and report observation overhead separately; any new drops require investigation before completion.

The final handoff includes both documents as repository files, the tested commands, the complete acceptance results, and any measured limitations. No claim of completion is based solely on host mocks or successful compilation.
