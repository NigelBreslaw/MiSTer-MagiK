# MiSTer frontend build policy

For local macOS UI design and deterministic RGB565 captures, see
[UI_PREVIEW.md](UI_PREVIEW.md).

Run focused local Rust checks through `scripts/cargo`. For everyday application
builds, deployment, testing and profiling, use the shared [2.0 workflow](../../magik2/README.md):

```sh
scripts/magik2 deploy
scripts/magik2 check
scripts/magik2 watch
scripts/magik2 check idle --profile
```

These commands target the development copy. No clean-commit or platform
qualification gate applies. The remaining sections describe retained legacy
platform/release builds, not prerequisites for application development.

## Platform/release builds

The typed build state machine is:

```text
Infer → Preflight → PrepareContainer → Compile → Verify → Receipt → Complete
```

`BuildSpec` infers profile, UI scope, production features, toolchain, artifact,
and cache identity from changed components and delivery intent. Apple Silicon
uses Apple `container`; Linux CI may use the explicitly typed cross adapter.
There is no automatic backend fallback.

Every artifact receipt records the exact clean source SHA, lock/toolchain/cache
identity, target, profile, features, and output digest. Runtime and platform
delivery reject mismatched or dirty receipts.

Use:

```text
$magik-rust-lsp
git add -- PATH...
git commit -m "Describe the change"
git push
scripts/agent deliver platform
```

Validation ownership is defined in root `AGENTS.md`; `scripts/agent plan`
previews the selected fast Python checks and CI boundary.
`deliver platform` owns build scope, artifact qualification, transport, activation,
rollback, and smoke verification. Ordinary attended launcher control uses
`scripts/agent device launcher restart --attended`. The fixed scene runners
were retired in milestone 9.

## Local FPGA signoff

Apple Silicon can run the complete matched FPGA signoff locally:

```text
QUARTUS_ACCEPT_EULA=1 scripts/agent fpga setup
scripts/agent fpga signoff
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
`scripts/agent build runtime-analysis` produces
the `release-device-profile` offline artifact and is not callable from the
benchmark workflow or deployable by it. Runtime delivery always
publishes `mister-magik-fb` together with its regenerated
`platform-v3.manifest`; no binary-only build or deployment command is exposed.
