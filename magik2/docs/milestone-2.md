# Mini-MagiK and the shared application workflow

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
| Real-app window smoke | Window appeared, but the original assertion missed the input-error banner; this is not controller acceptance |
| Real-app stream, idle benchmark and profile | Not run: further device tests stopped at the user's request |
| Reduced-restart fixture and input launch fix | Locally checked; not reapplied or rerun on the device |

Focused local checks passed: 49 host tests, four shared-support tests, the
native-service suite, the focused input-context check, and 36 CI-orchestration
unit tests. A pre-existing viewer shutdown test sometimes reports a Python
thread warning; its assertion passes. No broad local Rust assurance was run.

The input banner's cause was the missing `MISTER_MAGIK_INPUT_PROXY=1` and
`MISTER_MAGIK_INPUT_PROXY_PROTOCOL` launch settings. Saved Main status already
advertised protocol 2. The app's input reader exits early without these settings;
this was a launch-context omission, not a need to upgrade Main or a Python error.
The fix is committed in source. The currently running device app predates it;
applying it requires a service update and one application restart. No further
device operations were performed after the user's request to reduce testing.

## Remaining hardware acceptance

At the user's chosen time, apply the input-context fix once and check controller
input and one real-app stream. Run the real idle measurement or profile only
when requested, using the shared session fixture. Do not repeat the completed
transfer comparison or Mini-MagiK measurements. Do not call the milestone's
hardware acceptance complete until the real-app checks above have evidence.

Raw artifacts remain outside Git in `build/magik2-results/`. Relevant bundles:

- Mini deployment: `20260905T151743Z-b256c3ebcfc1`
- Mini motion/profile: `20260905T152023Z-574a58f1f9a5`
- Mini stream: `20260905T152233Z-03209bfea6d6`
- Real deployment: `20260905T152336Z-757dddb19891`
- Original real window smoke: `20260905T152527Z-39459760ea38`

[Saved transfer comparison](milestone-2-transfer-check.md).
