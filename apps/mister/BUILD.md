# MiSTer frontend build policy

For local macOS UI design and deterministic RGB565 captures, see
[UI_PREVIEW.md](UI_PREVIEW.md).

Run focused local Rust checks through `scripts/cargo`. For everyday application
builds, deployment, testing and profiling, use the shared [development workflow](../../magik/README.md):

```sh
scripts/magik deploy
scripts/magik check
scripts/magik watch
scripts/magik check idle --profile
```

These commands target the development copy. No clean-commit or platform
qualification gate applies. The remaining sections describe retained legacy
platform/release builds, not prerequisites for application development.

## Platform/release builds

Use `scripts/magik-ci build runtime-device` for the profile-enabled ARM runtime.
`scripts/magik-platform platform --help` and `local-main --help` describe explicit
artifact publication and Main delivery. Normal application iteration uses
`scripts/magik deploy`; a clean release receipt is not a development prerequisite.

`scripts/magik-ci plan` previews local checks. CI owns broad Rust/ARM validation.

## Local FPGA signoff

Apple Silicon can run the complete matched FPGA signoff locally:

```text
QUARTUS_ACCEPT_EULA=1 scripts/magik-platform fpga setup
scripts/magik-platform fpga signoff
```

Setup installs pinned Quartus Lite 17.0 Build 595 into the ignored local cache.
Quartus itself runs in an amd64 Apple container under Rosetta; the official
installer uses a QEMU amd64 chroot because it cannot complete under Rosetta.
Signoff reads the local `main` ref without switching the caller's worktree,
uses isolated generated source checkouts, builds the same stock, pinned
pre-observer, and final seed-1 variants as CI, and runs the same delta checker.
Rosetta requires Quartus parallel synthesis to be disabled; fitter and timing
remain parallel. That compatibility setting is applied identically to all
three variants and is part of the synthesis-cache identity.

Completed synthesis is cached per variant before the checker runs. A failing
signoff is therefore reproducible without another synthesis pass. Stock,
pre-observer, and final inputs have independent keys, so observer RTL changes
rebuild only the final variant and workflow or documentation-only commits do
not trigger synthesis. New results are built in staging directories and
promoted only after completion, preserving a valid cache across cancellation
or failure. Use an absolute `MISTER_FPGA_LOCAL_ROOT` shared by worktrees when
another local agent must reuse the install and completed variants. Local RBFs
remain diagnostic artifacts; only the GitHub platform workflow can publish or
qualify an RBF for deployment.

Compilation intent is explicit:

| Intent | Cargo policy | Artifact use |
| --- | --- | --- |
| Focused Rust tests, Clippy, checks, host CI | Dev/test, optimization 0, no debug info or LTO, incremental, 256 codegen units | Correctness only |
| Ordinary PR and main ARM CI | `ci-fast`, with the same compile-first settings | Linked non-production diagnostics |
| Runnable labs and captures | `release-live` or the owning device/profile release profile | Performance-sensitive iteration and evidence |
| Delivery | `release-device` runtime plus optimized manager | Installed development runtime |
| Alpha, beta, and release publication | `release-device` runtime plus optimized manager | Published production artifact |

`ci-fast` must never feed delivery, packaging, binary-size evidence, or a
release channel. Conversely, ordinary CI must not pay for release optimization;
its linked artifacts exist to prove the complete ARM application and agent
still link and satisfy their shared-library contracts.

Historical compile-policy measurements and reproduction inputs are retained in
`history/toolchain-bench/compile-policy-pre-push-20260807.json`; they do not
describe the current pre-push validation boundary.

The canonical `release-device` runtime uses the measured thin-LTO profile
(`opt-level=3`, thirty-two codegen units), includes dormant on-device profiling
support, and retains function symbols. Binary size is not a release gate;
device correctness, memory headroom, and frame cadence remain required.
Benchmarks activate profiling only on the already-installed runtime.
Offline profile analysis is available in Desktop. Platform publication includes its
verified manifest; ordinary development application delivery uses the native service.
