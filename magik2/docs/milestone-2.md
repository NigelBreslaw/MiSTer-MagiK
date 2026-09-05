# Mini-MagiK and the shared application workflow

> Review correction: the initial real-app deployment isolated the executable,
> but its runtime data still defaulted to production. Those earlier real-app
> measurements are historical tooling evidence, not a development-layout
> baseline. The corrected build now reports all application data under
> `/media/fat/mister-magik-dev` and selects `MiSTer_MagiKDev`. See the
> [careful review](milestone-2-review.md) for findings and limitations.


## Implemented

- Mini-MagiK remains the small consumer in `magik2/probe`, with binary name
  `mini-magik`. The real app remains in `apps/mister`.
- Two explicit app definitions select package, binary, features and profile.
  `deploy`, `build`, `watch` and `check` accept `--app mini-magik|magik`.
- Both apps use the same native upload, expected artifact hash, process
  ownership, readiness, test tunnel, preview publisher, measurement windows and
  CPU profile implementation. The shared Rust code is `crates/tooling-support`.
- The real app is an optional `magik2` feature and a separate development copy
  under `/media/fat/mister-magik2/magik`. Its existing renderer and committed
  latch framebuffer supply the observations; production installation, Main and
  platform files are not replaced.
- The real build reuses the existing device test profile, Cortex-A9 flags,
  minimal FFmpeg configuration and private font assets. Missing assets are
  initialized automatically; embedded font changes invalidate the build cache.
- Scenarios are also benchmarks. Mini-MagiK measures motion; the real app's
  initial workload is explicitly launcher idle. There are two ordinary
  measurement windows, with no percentile claims. Profiling is selected
  separately and excluded from ordinary performance aggregates.
- Ordinary scenarios now share one application session. `--profile` selects
  only the profile case. Repetitions do not restart the application. This was
  corrected after the user observed excessive restarts from the old fixture.
- Slint callbacks are serviced before the real launcher decides to sleep, so
  testing remains responsive when the UI has no frame to redraw.
- Main's advertised input-proxy settings are passed to the development child.
  The real-app smoke assertion now rejects the visible input-error banner.
- App switching clears cached preview frames and rejects frames from receivers
  that started before the switch. Tooling ownership includes the shared crate;
  CI checks the optional real-app feature as well as shared-code tests.

## Evidence collected on 5 September 2026

| Check | Evidence |
|---|---|
| Old/new saved transfer throughput | Exactly two attempts each; see the transfer report |
| Mini-MagiK ARM build and deployment | Passed |
| Real-app ARM build and separate deployment | Passed |
| Mini-MagiK smoke | Passed |
| Mini-MagiK motion | Two windows, 300 presentations each, zero physical drops and latch rejections |
| Mini-MagiK profile | Completed, 888 sampled stacks retained; 597 presentations, zero drops/rejections |
| Mini-MagiK native stream | Two successive decoded 960 × 540 frames |
| Real-app smoke | Passed with the input-error banner assertion; input launch settings applied |
| Real-app idle benchmark | Two 5,004 ms windows in the same PID (15815); zero redraws |
| Real-app profile | One 10,016 ms window; 13 samples, folded stacks and flamegraph retained |
| Real-app stream | Two successive decoded 960 × 540 frames; no app restart |
| Corrected real-app development layout | Deployed; running artifact hash and all reported data paths verified without another restart |
| Reduced-restart fixture | Smoke and both ordinary measurements shared one process; cleanup passed |

Focused local checks passed: 49 host tests, four shared-support tests, the
native-service suite, the focused input-context check, and 36 CI-orchestration
unit tests. A pre-existing viewer shutdown test sometimes reports a Python
thread warning; its assertion passes. No broad local Rust assurance was run.

The input banner's cause was the missing `MISTER_MAGIK_INPUT_PROXY=1` and
`MISTER_MAGIK_INPUT_PROXY_PROTOCOL` launch settings. Saved Main status already
advertised protocol 2. The app's input reader exits early without these settings;
this was a launch-context omission, not a need to upgrade Main or a Python error.
The fix was subsequently applied after the user clarified that necessary
restarts remain authorized.

The first resumed test connection expired because the real launcher serviced
Slint test callbacks only during drawing. With input fixed, its idle path no
longer drew repeatedly, exposing that scheduling bug. Callbacks now run before
the idle decision. That failed run produced no benchmark samples; the corrected
run passed smoke and exactly two idle windows in a single session.

## Completion and interpretation

The planned shared-workflow milestone is complete. Both apps have built,
deployed, streamed, run Python scenarios and produced a CPU profile through the
common tooling. The input-error banner check passed after the launch correction.
A physical controller button was not pressed by automation; the smoke check
verifies the app's input-health indication rather than claiming a manual
controller exercise.

The real app's benchmark is idle, so zero redraws are expected. Its 13 CPU
samples verify the profile pipeline but are too sparse for hotspot conclusions.
These idle results are not a real-versus-mini animation comparison. No additional
performance measurements or tuning were attempted. The real development app
remains running; production app, Main and platform files were not replaced.

The completed old/new transfer comparison was not repeated. Mini-MagiK's
completed measurements and profile were also retained without rerunning them.

Raw artifacts remain outside Git in `build/magik2-results/`. Relevant bundles:

- Mini deployment: `20260905T151743Z-b256c3ebcfc1`
- Mini motion/profile: `20260905T152023Z-574a58f1f9a5`
- Mini stream: `20260905T152233Z-03209bfea6d6`
- Real deployment: `20260905T152336Z-757dddb19891`
- Original real window smoke: `20260905T152527Z-39459760ea38`
- Resumed connection failure (no measurements): `20260905T154137Z-1be62da40eb2`
- Corrected real smoke/two idle windows: `20260905T154436Z-9eba19c5540d`
- Real profile: `20260905T154701Z-918cd46719e1`
- Real stream and final diagnostics: `20260905T154817Z-11c661ef1d53`

[Saved transfer comparison](milestone-2-transfer-check.md).


Development-layout correction bundles:

- Deployment: `20260905T160815Z-e1d086d0c5ce`
- Read-only running-artifact and path verification: `20260905T161135Z-c113eb76215f`

Earlier runs may have modified production runtime data. There is no pre-run
snapshot to enumerate those changes reliably; no speculative data rollback was
attempted. The statement above about production files not being replaced applies
to executable/platform delivery, not to runtime data isolation before this fix.
